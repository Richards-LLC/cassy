//! cas-8ca9: one stateful, in-process Cassy Cloud for the distributed factory
//! tests, so they run in CI without a real endpoint or token.
//!
//! It serves the routes the client uses, with petra-stella-cloud's shapes:
//! - agents: `POST /api/agents/register` (echoes `{status, agent}`),
//!   `GET /api/agents` (scoped by user only, never by project),
//!   `POST /api/agents/{id}/heartbeat` (the bare agent),
//!   `POST /api/agents/{id}/shutdown`, `GET /api/agents/locks`,
//!   `GET /api/agents/{id}/locks`;
//! - task claims: `POST /api/agents/tasks/{key}/{claim,release,renew}` and
//!   `GET /api/agents/tasks/{key}/lock` (409 with the owner on conflict);
//! - the peer mailbox of petra-stella-cloud#152: `POST /api/peer-messages`
//!   (sender and recipient must be registered, the recipient under the same
//!   repo; sender identity stamped from the registry; `dedupe_key`
//!   collapses resends), `/claim`, `/ack` and `GET /api/peer-messages/{id}`.
//!
//! Served by `tiny_http` on a thread, so sync and async tests both use it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    pub agents: BTreeMap<String, Value>,
    pub locks: BTreeMap<String, Value>,
    pub messages: Vec<Value>,
}

pub(super) struct FakeCloud {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    server: Arc<tiny_http::Server>,
}

impl Drop for FakeCloud {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

impl FakeCloud {
    pub fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("fake cloud binds"));
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let state: Arc<Mutex<State>> = Arc::default();
        let (thread_server, thread_state) = (Arc::clone(&server), Arc::clone(&state));
        std::thread::spawn(move || {
            for mut request in thread_server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let (status, reply) = handle(
                    &mut thread_state.lock().unwrap(),
                    request.method().as_str(),
                    request.url(),
                    &body,
                );
                let response = tiny_http::Response::from_string(reply.to_string())
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    );
                let _ = request.respond(response);
            }
        });
        Self { url, state, server }
    }

    /// A cloud config for this fake.
    pub fn config(&self) -> cas::cloud::CloudConfig {
        cas::cloud::CloudConfig {
            endpoint: self.url.clone(),
            token: Some("test-token".to_string()),
            ..Default::default()
        }
    }

    /// Point a machine's project at this cloud under `canonical_id`.
    pub fn seed(&self, cas_dir: &std::path::Path, canonical_id: &str) {
        self.config().save_to_cas_dir(cas_dir).unwrap();
        std::fs::write(
            cas_dir.join("config.toml"),
            format!("[project]\ncanonical_id = \"{canonical_id}\"\n"),
        )
        .unwrap();
    }

    /// Set an agent's last heartbeat `minutes` ago.
    pub fn age_heartbeat(&self, agent_id: &str, minutes: i64) {
        let at = (Utc::now() - Duration::minutes(minutes)).to_rfc3339();
        self.state.lock().unwrap().agents.get_mut(agent_id).unwrap()["last_heartbeat"] = json!(at);
    }

    /// The peer messages addressed to `recipient`.
    pub fn messages_to(&self, recipient: &str) -> Vec<Value> {
        self.state
            .lock()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m["recipient_agent_id"] == recipient)
            .cloned()
            .collect()
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn lock_json(key: &str, lock: &Value) -> Value {
    json!({
        "id": format!("lock-{key}"),
        "task_id": key,
        "agent_id": lock["agent_id"],
        "status": lock["status"],
        "expires_at": lock["expires_at"],
        "renewed_at": lock["renewed_at"],
        "renewal_count": lock["renewal_count"],
        "epoch": 1,
        "claim_reason": lock["claim_reason"],
    })
}

fn active_locks<'a>(state: &'a State, owner: Option<&str>) -> Vec<Value> {
    state
        .locks
        .iter()
        .filter(|(_, lock)| lock["status"] == "active")
        .filter(|(_, lock)| owner.is_none_or(|owner| lock["agent_id"] == owner))
        .map(|(key, lock)| lock_json(key, lock))
        .collect()
}

fn handle(state: &mut State, method: &str, url: &str, body: &Value) -> (u16, Value) {
    let path = url.split('?').next().unwrap_or(url);
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("POST", ["api", "agents", "register"]) => {
            let id = body["id"].as_str().unwrap_or_default().to_string();
            let agent = json!({
                "id": id,
                "name": body["name"],
                "agent_type": body["agent_type"],
                "status": "active",
                "pid": body["pid"],
                "session_id": null,
                "machine_id": body["machine_id"],
                "last_heartbeat": now(),
                "active_tasks": 0,
                "metadata": body["metadata"],
            });
            state.agents.insert(id, agent.clone());
            (200, json!({"status": "success", "agent": agent}))
        }
        ("GET", ["api", "agents"]) => {
            let agents: Vec<Value> = state.agents.values().cloned().collect();
            (
                200,
                json!({"status": "success", "agents": agents,
                         "next_cursor": null, "next_cursor_id": null}),
            )
        }
        ("GET", ["api", "agents", "locks"]) => (
            200,
            json!({"status": "success", "locks": active_locks(state, None)}),
        ),
        ("GET", ["api", "agents", id, "locks"]) => (
            200,
            json!({"status": "success", "locks": active_locks(state, Some(id))}),
        ),
        ("POST", ["api", "agents", id, "heartbeat"]) => match state.agents.get_mut(*id) {
            Some(agent) => {
                agent["last_heartbeat"] = json!(now());
                agent["status"] = json!("active");
                (200, agent.clone())
            }
            None => (404, json!({"error": "agent not found"})),
        },
        ("POST", ["api", "agents", id, "shutdown"]) => {
            if let Some(agent) = state.agents.get_mut(*id) {
                agent["status"] = json!("shutdown");
            }
            let mut released = 0;
            for lock in state.locks.values_mut() {
                if lock["agent_id"] == *id && lock["status"] == "active" {
                    lock["status"] = json!("released");
                    released += 1;
                }
            }
            (
                200,
                json!({"status": "success", "released_locks": released}),
            )
        }
        (_, ["api", "agents", "tasks", key, action]) => {
            claim_route(state, method, key, action, body)
        }
        ("POST", ["api", "peer-messages"]) => send(state, body),
        ("POST", ["api", "peer-messages", "claim"]) => {
            let recipients: Vec<&str> = body["recipient_agent_ids"]
                .as_array()
                .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut claimed = Vec::new();
            for message in state.messages.iter_mut() {
                let mine =
                    recipients.contains(&message["recipient_agent_id"].as_str().unwrap_or(""));
                if mine && message["status"] == "queued" {
                    message["status"] = json!("leased");
                    message["attempts"] = json!(message["attempts"].as_u64().unwrap_or(0) + 1);
                    claimed.push(message.clone());
                }
            }
            (200, json!({"messages": claimed}))
        }
        ("POST", ["api", "peer-messages", "ack"]) => {
            for ack in body["acks"].as_array().cloned().unwrap_or_default() {
                if let Some(message) = state.messages.iter_mut().find(|m| m["id"] == ack["id"]) {
                    message["status"] = ack["outcome"].clone();
                    message["delivered_at"] = json!(now());
                }
            }
            (200, json!({"status": "success"}))
        }
        ("GET", ["api", "peer-messages", id]) => {
            match state.messages.iter().find(|m| m["id"] == *id) {
                Some(m) => (
                    200,
                    json!({"id": m["id"], "status": m["status"],
                                    "delivered_at": m["delivered_at"], "attempts": m["attempts"]}),
                ),
                None => (404, json!({"error": "not found"})),
            }
        }
        _ => (404, json!({"error": "not found"})),
    }
}

fn claim_route(
    state: &mut State,
    method: &str,
    key: &str,
    action: &str,
    body: &Value,
) -> (u16, Value) {
    let agent_id = body["agent_id"].as_str().unwrap_or_default().to_string();
    let active = state
        .locks
        .get(key)
        .filter(|lock| lock["status"] == "active")
        .cloned();
    let duration = body["duration_secs"].as_i64().unwrap_or(600).clamp(1, 3600);
    match (method, action) {
        ("POST", "claim") => match active {
            // As petra-stella-cloud today: an active row is held whatever its
            // expires_at (petra-stella-cloud#150).
            Some(lock) if lock["agent_id"] != agent_id.as_str() => (
                409,
                json!({"status": "conflict", "result": null, "lock": null,
                       "error": "already_claimed", "owner_agent_id": lock["agent_id"]}),
            ),
            Some(lock) => (
                200,
                json!({"status": "success", "result": "claimed",
                                       "lock": lock_json(key, &lock), "error": null,
                                       "owner_agent_id": null}),
            ),
            None => {
                let lock = json!({
                    "agent_id": agent_id,
                    "status": "active",
                    "expires_at": (Utc::now() + Duration::seconds(duration)).to_rfc3339(),
                    "renewed_at": now(),
                    "renewal_count": 0,
                    "claim_reason": body["reason"],
                });
                let reply = lock_json(key, &lock);
                state.locks.insert(key.to_string(), lock);
                (
                    200,
                    json!({"status": "success", "result": "claimed", "lock": reply,
                             "error": null, "owner_agent_id": null}),
                )
            }
        },
        ("POST", "release") => {
            if let Some(lock) = state.locks.get_mut(key)
                && lock["status"] == "active"
                && lock["agent_id"] == agent_id.as_str()
            {
                lock["status"] = json!("released");
            }
            (200, json!({"status": "success"}))
        }
        ("POST", "renew") => match state.locks.get_mut(key) {
            Some(lock) if lock["status"] == "active" && lock["agent_id"] == agent_id.as_str() => {
                lock["expires_at"] = json!((Utc::now() + Duration::seconds(duration)).to_rfc3339());
                lock["renewed_at"] = json!(now());
                lock["renewal_count"] = json!(lock["renewal_count"].as_u64().unwrap_or(0) + 1);
                (
                    200,
                    json!({"status": "success", "lock": lock_json(key, lock)}),
                )
            }
            _ => (404, json!({"error": "No active lock found"})),
        },
        ("GET", "lock") => (
            200,
            json!({"lock": active.map(|lock| lock_json(key, &lock))}),
        ),
        _ => (404, json!({"error": "not found"})),
    }
}

fn send(state: &mut State, body: &Value) -> (u16, Value) {
    let (Some(sender), Some(recipient)) = (
        state
            .agents
            .get(body["sender_agent_id"].as_str().unwrap_or("")),
        state
            .agents
            .get(body["recipient_agent_id"].as_str().unwrap_or("")),
    ) else {
        return (404, json!({"error": "unknown agent"}));
    };
    let project = body["project_id"].as_str().unwrap_or("");
    let recipient_repo = recipient["metadata"]["canonical_id"].as_str().unwrap_or("");
    if !cas::cloud::project_ids_match_with_aliases(recipient_repo, project, &[]) {
        return (403, json!({"error": "recipient is not in this project"}));
    }
    if let Some(existing) = state
        .messages
        .iter()
        .find(|m| m["dedupe_key"] == body["dedupe_key"])
    {
        return (200, json!({"id": existing["id"], "status": "duplicate"}));
    }
    let id = format!("pm_{}", state.messages.len() + 1);
    let message = json!({
        "id": id,
        "project_id": project,
        "recipient_agent_id": body["recipient_agent_id"],
        "sender_agent_id": body["sender_agent_id"],
        "sender_name": sender["name"],
        "sender_machine_id": sender["machine_id"],
        "sender_session": sender["metadata"]["factory_session"],
        "body": body["body"],
        "summary": body["summary"],
        "in_reply_to": body["in_reply_to"],
        "dedupe_key": body["dedupe_key"],
        "created_at": now(),
        "attempts": 0,
        "status": "queued",
        "delivered_at": null,
    });
    state.messages.push(message);
    (200, json!({"id": id, "status": "queued"}))
}
