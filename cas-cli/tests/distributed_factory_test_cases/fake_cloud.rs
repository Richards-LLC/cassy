//! cas-8ca9: one stateful, in-process Cassy Cloud for the distributed factory
//! tests, so they run in CI without a real endpoint or token.
//!
//! cas-c7c7: it answers like production (petra-stella-cloud after PR #153), as
//! recorded by `scripts/cloud-contract-capture.py` into
//! `tests/fixtures/cloud_contract/`, and `contract_parity.rs` replays every
//! recorded exchange against it. In short:
//! - agents: `POST /api/agents/register` (`{status, agent}`), `GET /api/agents`
//!   (`?project=`, `?status=`, keyset paging newest-first with `limit`,
//!   `cursor`, `cursor_id`; `next_*` while a page is full), heartbeat (the bare
//!   agent; `{"metadata":{..}}` merged), shutdown (`{released_locks}`), and
//!   account and agent lock lists. `status` is computed live: `stale` once the
//!   heartbeat is five minutes old.
//! - task claims: an expired lock is free (epoch + 1); a live one refuses with
//!   409 `{status:"conflict", result, lock, error, owner_agent_id, agent_id,
//!   expires_at}` naming the holder; a same-agent re-claim does not extend;
//!   renew of a lost lock is 404; release is idempotent.
//! - the peer mailbox: send refuses in order `invalid_request` (400),
//!   `sender_not_owned` (403), `recipient_not_found` (404, including another or
//!   unknown project), `invalid_in_reply_to` (400); a reused `dedupe_key`
//!   answers `duplicate` with the original id; ids are UUIDs. Claim leases to a
//!   consumer (`lease_secs` 30-600); ack takes UUIDs only and answers per item
//!   200, 404 (unknown) or 409 (not leased to this consumer).
//!
//! The clock can be moved forward ([`FakeCloud::advance_clock`]) so liveness
//! and lock expiry need no sleeping. Served by `tiny_http` on a thread, so sync
//! and async tests both use it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    pub agents: BTreeMap<String, Value>,
    /// Explicit sandbox tokens and project memberships for two-account tests.
    pub accounts: BTreeMap<String, String>,
    pub team_projects: BTreeMap<String, (String, Vec<String>)>,
    pub locks: BTreeMap<String, Value>,
    pub messages: Vec<Value>,
    /// Seconds the fake's clock runs ahead of real time ([`FakeCloud::advance_clock`]).
    pub clock_offset_secs: i64,
    /// Answers to give instead of serving: (method, path fragment, status,
    /// times left), as production's rate limiter does ([`FakeCloud::inject_failure`]).
    pub injected: Vec<(String, String, u16, u32)>,
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
                let mut state = thread_state.lock().unwrap();
                let method = request.method().as_str().to_string();
                let url = request.url().to_string();
                let injected = state.injected.iter_mut().find(|(m, fragment, _, left)| {
                    *left > 0 && *m == method && url.contains(fragment.as_str())
                });
                let (status, reply) = match injected {
                    Some(failure) => {
                        failure.3 -= 1;
                        (failure.2, json!({"error": "Too many requests"}))
                    }
                    None => {
                        let token = request
                            .headers()
                            .iter()
                            .find(|h| h.field.equiv("Authorization"))
                            .map(|h| h.value.as_str().trim_start_matches("Bearer "))
                            .unwrap_or("");
                        let user = state
                            .accounts
                            .get(token)
                            .cloned()
                            .unwrap_or_else(|| USER.into());
                        handle(&mut state, &method, &url, &body, &user)
                    }
                };
                drop(state);
                let mut response = tiny_http::Response::from_string(reply.to_string())
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    );
                if status == 429 {
                    // Clients honour Retry-After; zero keeps a test's retries
                    // without the default backoff.
                    response.add_header(
                        tiny_http::Header::from_bytes(&b"Retry-After"[..], &b"0"[..]).unwrap(),
                    );
                }
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
        let mut state = self.state.lock().unwrap();
        let at = stamp(state.now() - Duration::minutes(minutes));
        state.agents.get_mut(agent_id).unwrap()["last_heartbeat"] = json!(at);
    }

    /// Move the fake's clock `secs` forward: liveness, lock expiry and new
    /// timestamps all read it, so a test need not sleep.
    pub fn advance_clock(&self, secs: i64) {
        self.state.lock().unwrap().clock_offset_secs += secs;
    }

    /// Answer the next `times` `method` requests whose URL contains
    /// `fragment` with `status`, without serving them (e.g. a 429 from
    /// production's rate limiter).
    pub fn inject_failure(&self, method: &str, fragment: &str, status: u16, times: u32) {
        self.state
            .lock()
            .unwrap()
            .injected
            .push((method.to_string(), fragment.to_string(), status, times));
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

/// The single account this fake's bearer token belongs to.
const USER: &str = "test-user";
/// An agent whose heartbeat is older than this reads `stale` (production).
const LIVE_SECS: i64 = 300;

impl State {
    fn now(&self) -> DateTime<Utc> {
        Utc::now() + Duration::seconds(self.clock_offset_secs)
    }
}

/// Production's timestamp shape: fixed microseconds, `Z`, so stored values
/// also sort as strings.
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn parse(at: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at.as_str()?).ok().map(|at| at.with_timezone(&Utc))
}

fn query_params(url: &str) -> Vec<(String, String)> {
    url.split_once('?')
        .map(|(_, query)| {
            url::form_urlencoded::parse(query.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// The agent as production renders it: internal `_`-prefixed fields dropped
/// and `status` computed live.
fn render_agent(state: &State, agent: &Value) -> Value {
    let mut out = agent.clone();
    let status = if agent["_shutdown"].as_bool() == Some(true) {
        "shutdown"
    } else if parse(&agent["last_heartbeat"])
        .is_some_and(|at| (state.now() - at).num_seconds() > LIVE_SECS)
    {
        "stale"
    } else {
        "active"
    };
    if let Some(map) = out.as_object_mut() {
        map.retain(|key, _| !key.starts_with('_'));
        map.insert("status".into(), json!(status));
    }
    out
}

fn lock_json(key: &str, lock: &Value) -> Value {
    json!({
        "id": lock["id"],
        "task_id": lock["task_id"].as_str().unwrap_or(key),
        "user_id": lock["user_id"].as_str().unwrap_or(USER),
        "project_id": lock["project_id"],
        "team_id": lock["team_id"],
        "agent_id": lock["agent_id"],
        "status": lock["status"],
        "expires_at": lock["expires_at"],
        "renewed_at": lock["renewed_at"],
        "renewal_count": lock["renewal_count"],
        "epoch": lock["epoch"],
        "claim_reason": lock["claim_reason"],
    })
}

/// A lock that is held: active and not yet expired.
fn is_live(state: &State, lock: &Value) -> bool {
    lock["status"] == "active" && parse(&lock["expires_at"]).is_some_and(|at| at > state.now())
}

fn live_locks(state: &State, owner: Option<&str>, user: &str, project: Option<&str>) -> Vec<Value> {
    state
        .locks
        .iter()
        .filter(|(_, lock)| is_live(state, lock))
        .filter(|(_, lock)| visible_lock(state, lock, user, project))
        .filter(|(_, lock)| owner.is_none_or(|owner| lock["agent_id"] == owner))
        .map(|(key, lock)| lock_json(key, lock))
        .collect()
}

fn handle(state: &mut State, method: &str, url: &str, body: &Value, user: &str) -> (u16, Value) {
    let path = url.split('?').next().unwrap_or(url);
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("POST", ["api", "agents", "register"]) => register(state, body, user),
        ("GET", ["api", "agents"]) => list_agents(state, url, user),
        ("GET", ["api", "agents", "locks"]) => (
            200,
            json!({"status": "success", "locks": live_locks(state, None, user, query_params(url).iter().find(|(k, _)| k == "project").map(|(_, v)| v.as_str()))}),
        ),
        ("GET", ["api", "agents", id, "locks"]) => (
            200,
            json!({"status": "success", "locks": live_locks(state, Some(id), user, None)}),
        ),
        ("POST", ["api", "agents", id, "heartbeat"]) => {
            let now = stamp(state.now());
            let Some(agent) = state.agents.get_mut(*id) else {
                return (404, json!({"error": "Agent not found"}));
            };
            agent["last_heartbeat"] = json!(now);
            // An optional `{"metadata": {...}}` body is merged: canonical_id
            // re-binds the project, null clears a key (PR #153).
            if let Some(patch) = body.get("metadata").and_then(Value::as_object) {
                for (key, value) in patch {
                    if value.is_null() {
                        if let Some(metadata) = agent["metadata"].as_object_mut() {
                            metadata.remove(key);
                        }
                    } else {
                        agent["metadata"][key] = value.clone();
                    }
                }
                agent["project_id"] = agent["metadata"]["canonical_id"].clone();
            }
            let agent = agent.clone();
            (200, render_agent(state, &agent))
        }
        ("POST", ["api", "agents", id, "shutdown"]) => {
            if let Some(agent) = state.agents.get_mut(*id) {
                agent["_shutdown"] = json!(true);
            }
            let mut released = 0;
            let now = state.now();
            for lock in state.locks.values_mut() {
                let live = lock["status"] == "active" && parse(&lock["expires_at"]).is_some_and(|at| at > now);
                if live && lock["agent_id"] == *id {
                    lock["status"] = json!("released");
                    released += 1;
                }
            }
            (200, json!({"released_locks": released}))
        }
        (_, ["api", "agents", "tasks", key, action]) => {
            claim_route(state, method, key, action, body, user)
        }
        ("POST", ["api", "peer-messages"]) => send(state, body),
        ("POST", ["api", "peer-messages", "claim"]) => claim_messages(state, body),
        ("POST", ["api", "peer-messages", "ack"]) => ack(state, body),
        ("GET", ["api", "peer-messages", id]) => match state.messages.iter().find(|m| m["id"] == *id) {
            Some(m) => (
                200,
                json!({"id": m["id"], "status": m["status"],
                       "delivered_at": m["delivered_at"], "attempts": m["attempts"]}),
            ),
            None => (404, json!({"error": "Peer message not found"})),
        },
        _ => (404, json!({"error": "not found"})),
    }
}

fn register(state: &mut State, body: &Value, user: &str) -> (u16, Value) {
    let id = body["id"].as_str().unwrap_or_default().to_string();
    let now = stamp(state.now());
    let created = state
        .agents
        .get(&id)
        .map(|agent| agent["_created_at"].clone())
        .unwrap_or_else(|| json!(stamp(state.now())));
    let agent = json!({
        "id": id,
        "name": body["name"],
        "agent_type": body["agent_type"],
        "user_id": user,
        "project_id": body["metadata"]["canonical_id"],
        "team_id": team_for(state, user, body["metadata"]["canonical_id"].as_str()).map(|(team, _)| team),
        "pid": body["pid"],
        "session_id": null,
        "machine_id": body["machine_id"],
        "last_heartbeat": now,
        "active_tasks": 0,
        "metadata": body["metadata"],
        "_created_at": created,
    });
    state.agents.insert(id, agent.clone());
    (200, json!({"status": "success", "agent": render_agent(state, &agent)}))
}

/// `GET /api/agents`: newest first by registration, keyset-paged.
fn list_agents(state: &State, url: &str, user: &str) -> (u16, Value) {
    let params = query_params(url);
    let param = |name: &str| params.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    let project = param("project");
    let status = param("status");
    let limit = param("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    let cursor = param("cursor").zip(param("cursor_id"));
    let mut agents: Vec<(String, String, Value)> = state
        .agents
        .values()
        .filter(|agent| match &project {
            // `?project=<canonical id or alias>` lists that project's agents,
            // teammates included (PR #153).
            Some(project) => {
                (agent["user_id"] == user || team_for(state, user, Some(project)).is_some())
                    && cas::cloud::project_ids_match_with_aliases(
                        agent["metadata"]["canonical_id"].as_str().unwrap_or(""),
                        project,
                        &[],
                    )
            }
            // Without it, only the caller's own agents.
            None => agent["user_id"].as_str().is_none_or(|id| id == user),
        })
        .map(|agent| render_agent(state, agent))
        .filter(|agent| status.as_deref().is_none_or(|status| agent["status"] == status))
        .map(|agent| {
            let id = agent["id"].as_str().unwrap_or_default().to_string();
            let created = state.agents[&id]["_created_at"].as_str().unwrap_or_default().to_string();
            (created, id, agent)
        })
        .collect();
    agents.sort_by(|a, b| (&b.0, &b.1).cmp(&(&a.0, &a.1)));
    let page: Vec<(String, String, Value)> = agents
        .into_iter()
        .filter(|(created, id, _)| match &cursor {
            Some((at, at_id)) => (created, id) < (at, at_id),
            None => true,
        })
        .take(limit)
        .collect();
    let next = (page.len() == limit).then(|| page.last().cloned()).flatten();
    let (next_cursor, next_id) = match &next {
        Some((created, id, _)) => (json!(created), json!(id)),
        None => (Value::Null, Value::Null),
    };
    (
        200,
        json!({
            "status": "success",
            "agents": page.into_iter().map(|(_, _, agent)| agent).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
            "next_cursor_id": next_id,
            "next_sort_cursor": next_cursor,
        }),
    )
}

fn team_for<'a>(
    state: &'a State,
    user: &str,
    project: Option<&str>,
) -> Option<&'a (String, Vec<String>)> {
    let project = project?;
    state
        .team_projects
        .iter()
        .find(|(id, (_, members))| {
            cas::cloud::project_ids_match_with_aliases(id, project, &[])
                && members.iter().any(|m| m == user)
        })
        .map(|(_, team)| team)
}

fn visible_lock(state: &State, lock: &Value, user: &str, project: Option<&str>) -> bool {
    let owner = lock["user_id"].as_str().unwrap_or(USER);
    match project {
        None => owner == user,
        Some(project) => {
            lock["project_id"]
                .as_str()
                .is_some_and(|id| cas::cloud::project_ids_match_with_aliases(id, project, &[]))
                && (owner == user
                    || team_for(state, user, Some(project))
                        .is_some_and(|(team, _)| lock["team_id"] == *team))
        }
    }
}

fn claim_route(
    state: &mut State,
    method: &str,
    key: &str,
    action: &str,
    body: &Value,
    user: &str,
) -> (u16, Value) {
    let agent_id = body["agent_id"].as_str().unwrap_or_default().to_string();
    let duration = body["duration_secs"].as_i64().unwrap_or(600).clamp(1, 3600);
    let now = state.now();
    let project = body["canonical_id"]
        .as_str()
        .or_else(|| {
            state
                .agents
                .get(&agent_id)
                .and_then(|a| a["project_id"].as_str())
        })
        .map(|p| cas::cloud::canonical_project_id(p).unwrap_or_else(|| p.into()));
    let team = team_for(state, user, project.as_deref()).map(|(team, _)| team.clone());
    let own_key = if user == USER {
        key.into()
    } else {
        format!("{user}:{key}")
    };
    let held = state
        .locks
        .iter()
        .find(|(storage, lock)| {
            let task = lock["task_id"].as_str().unwrap_or(storage);
            task == key
                && is_live(state, lock)
                && (lock["user_id"].as_str().unwrap_or(USER) == user
                    || (action == "claim"
                        && team.as_ref().is_some_and(|t| lock["team_id"] == *t)
                        && lock["project_id"].as_str() == project.as_deref()))
        })
        .map(|(k, v)| (k.clone(), v.clone()));
    if method == "POST"
        && state
            .agents
            .get(&agent_id)
            .is_some_and(|a| a["user_id"] != user)
    {
        return (404, json!({"error": "Agent not found"}));
    }
    match (method, action) {
        ("POST", "claim") => match held {
            Some((_, lock)) if lock["agent_id"] != agent_id.as_str() => (
                409,
                json!({"status": "conflict", "result": null, "lock": null,
                       "error": "already_claimed", "owner_agent_id": lock["agent_id"],
                       "agent_id": lock["agent_id"], "expires_at": lock["expires_at"],
                       "user_id": lock["user_id"].as_str().unwrap_or(USER)}),
            ),
            Some((_, lock)) => (
                200,
                json!({"status": "success", "result": "claimed", "lock": lock_json(key, &lock),
                       "error": null, "owner_agent_id": null}),
            ),
            None => {
                let epoch = state
                    .locks
                    .get(&own_key)
                    .and_then(|lock| lock["epoch"].as_u64())
                    .unwrap_or(0)
                    + 1;
                let lock = json!({
                    "id": uuid::Uuid::new_v4().to_string(), "task_id": key,
                    "agent_id": agent_id, "user_id": user, "project_id": project, "team_id": team,
                    "status": "active", "expires_at": stamp(now + Duration::seconds(duration)),
                    "renewed_at": stamp(now), "renewal_count": 0, "epoch": epoch, "claim_reason": body["reason"],
                });
                let reply = lock_json(key, &lock);
                state.locks.insert(own_key, lock);
                (200, json!({"status": "success", "result": "claimed", "lock": reply,
                             "error": null, "owner_agent_id": null}))
            }
        },
        ("POST", "release") => {
            if let Some((storage, lock)) = held
                && lock["agent_id"] == agent_id
            {
                state.locks.get_mut(&storage).unwrap()["status"] = json!("released");
            }
            (200, json!({"status": "success"}))
        }
        ("POST", "renew") => match state.locks.get_mut(&own_key) {
            Some(lock) if lock["status"] == "active" && lock["agent_id"] == agent_id.as_str() => {
                lock["expires_at"] = json!(stamp(now + Duration::seconds(duration)));
                lock["renewed_at"] = json!(stamp(now));
                lock["renewal_count"] = json!(lock["renewal_count"].as_u64().unwrap_or(0) + 1);
                (200, json!({"lock": lock_json(key, lock)}))
            }
            _ => (404, json!({"error": "No active lock found"})),
        },
        ("GET", "lock") => (
            200,
            json!({"lock": held.map(|(_, lock)| lock_json(key, &lock))}),
        ),
        _ => (404, json!({"error": "not found"})),
    }
}

fn invalid(code: &str, error: &str) -> (u16, Value) {
    (400, json!({"error": error, "code": code}))
}

/// A non-empty string of at most `max` characters.
fn bounded(value: &Value, max: usize) -> bool {
    value.as_str().is_some_and(|text| !text.is_empty() && text.chars().count() <= max)
}

fn send(state: &mut State, body: &Value) -> (u16, Value) {
    for (field, max) in [
        ("project_id", 200),
        ("sender_agent_id", 200),
        ("recipient_agent_id", 200),
        ("dedupe_key", 200),
        ("body", 64000),
    ] {
        if !bounded(&body[field], max) {
            return invalid(
                "invalid_request",
                &format!("{field} must be a non-empty string of at most {max} characters"),
            );
        }
    }
    for (field, max) in [("summary", 500), ("in_reply_to", 200)] {
        if !body[field].is_null() && !bounded(&body[field], max) {
            return invalid(
                "invalid_request",
                &format!("{field} must be a non-empty string of at most {max} characters"),
            );
        }
    }
    let Some(sender) = state.agents.get(body["sender_agent_id"].as_str().unwrap_or("")).cloned() else {
        return (403, json!({"error": "sender_agent_id is not an agent of the caller",
                            "code": "sender_not_owned"}));
    };
    if let Some(existing) = state.messages.iter().find(|m| m["dedupe_key"] == body["dedupe_key"]) {
        return (200, json!({"id": existing["id"], "status": "duplicate"}));
    }
    let project = body["project_id"].as_str().unwrap_or("");
    let addressable = state
        .agents
        .get(body["recipient_agent_id"].as_str().unwrap_or(""))
        .is_some_and(|recipient| {
            cas::cloud::project_ids_match_with_aliases(
                recipient["metadata"]["canonical_id"].as_str().unwrap_or(""),
                project,
                &[],
            )
        });
    if !addressable {
        return (404, json!({"error": "recipient_agent_id is not an addressable agent of this project",
                            "code": "recipient_not_found"}));
    }
    if !body["in_reply_to"].is_null() && !state.messages.iter().any(|m| m["id"] == body["in_reply_to"]) {
        return invalid("invalid_in_reply_to", "in_reply_to must be a peer message id");
    }
    let id = uuid::Uuid::new_v4().to_string();
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
        "created_at": stamp(state.now()),
        "attempts": 0,
        "status": "queued",
        "delivered_at": null,
        "leased_by": null,
        "lease_until": null,
    });
    state.messages.push(message);
    (200, json!({"id": id, "status": "queued"}))
}

/// The message as a claim returns it.
fn claimed_view(message: &Value) -> Value {
    let mut view = serde_json::Map::new();
    for key in [
        "id", "project_id", "recipient_agent_id", "sender_agent_id", "sender_name",
        "sender_machine_id", "sender_session", "body", "summary", "in_reply_to", "created_at",
        "attempts",
    ] {
        view.insert(key.into(), message[key].clone());
    }
    Value::Object(view)
}

fn ids(value: &Value) -> Option<Vec<String>> {
    let list = value.as_array()?;
    if list.is_empty() || list.len() > 50 {
        return None;
    }
    list.iter().map(|id| id.as_str().map(str::to_string)).collect()
}

fn claim_messages(state: &mut State, body: &Value) -> (u16, Value) {
    let Some(projects) = ids(&body["project_ids"]) else {
        return invalid("invalid_request", "project_ids must be an array of 1 to 50 ids");
    };
    let Some(recipients) = ids(&body["recipient_agent_ids"]) else {
        return invalid("invalid_request", "recipient_agent_ids must be an array of 1 to 50 ids");
    };
    let Some(consumer) = body["consumer_id"].as_str().filter(|id| !id.is_empty()) else {
        return invalid("invalid_request", "consumer_id must be a non-empty string");
    };
    let max = body["max"].as_i64().unwrap_or(50);
    if !(1..=50).contains(&max) {
        return invalid("invalid_request", "max must be an integer from 1 to 50");
    }
    let lease = body["lease_secs"].as_i64().unwrap_or(120);
    if !(30..=600).contains(&lease) {
        return invalid("invalid_request", "lease_secs must be an integer from 30 to 600");
    }
    let now = state.now();
    let mut claimed = Vec::new();
    for message in state.messages.iter_mut() {
        if claimed.len() as i64 >= max {
            break;
        }
        let mine = recipients.iter().any(|id| message["recipient_agent_id"] == id.as_str());
        let in_project = projects.iter().any(|project| {
            cas::cloud::project_ids_match_with_aliases(message["project_id"].as_str().unwrap_or(""), project, &[])
        });
        let free = message["status"] == "queued"
            || (message["status"] == "leased" && parse(&message["lease_until"]).is_some_and(|at| at <= now));
        if mine && in_project && free {
            message["status"] = json!("leased");
            message["leased_by"] = json!(consumer);
            message["lease_until"] = json!(stamp(now + Duration::seconds(lease)));
            message["attempts"] = json!(message["attempts"].as_u64().unwrap_or(0) + 1);
            claimed.push(claimed_view(message));
        }
    }
    (200, json!({"messages": claimed, "denied": []}))
}

fn ack(state: &mut State, body: &Value) -> (u16, Value) {
    let consumer = body["consumer_id"].as_str().unwrap_or("");
    let acks = body["acks"].as_array().cloned().unwrap_or_default();
    if acks.iter().any(|ack| ack["id"].as_str().and_then(|id| uuid::Uuid::parse_str(id).ok()).is_none()) {
        return invalid("invalid_request", "ack id must be a UUID");
    }
    let now = stamp(state.now());
    let results: Vec<Value> = acks
        .iter()
        .map(|ack| {
            let status = match state.messages.iter_mut().find(|m| m["id"] == ack["id"]) {
                None => 404,
                Some(message) if message["status"] == "leased" && message["leased_by"] == consumer => {
                    message["status"] = ack["outcome"].clone();
                    message["delivered_at"] = json!(now);
                    200
                }
                Some(_) => 409,
            };
            json!({"id": ack["id"], "status": status})
        })
        .collect();
    (200, json!({"results": results}))
}

#[test]
fn cas_8ea7_personal_claims_remain_account_private() {
    use cas::cloud::CloudCoordinator;
    use cas::types::Agent;
    let cloud = FakeCloud::start();
    let coordinator = |user: &str| {
        cloud.state.lock().unwrap().accounts.insert(format!("token-{user}"), user.into());
        let mut config = cloud.config();
        config.token = Some(format!("token-{user}"));
        let mut c = CloudCoordinator::new(config)
            .unwrap()
            .with_canonical_id(Some("github.com/acme/personal".into()));
        c.register(&Agent::new(user.into(), format!("supervisor-{user}")))
            .unwrap();
        c
    };
    let a = coordinator("a");
    let b = coordinator("b");
    assert!(a.claim("cas-personal", 600, None).unwrap().is_success());
    assert!(b.list_locks().unwrap().is_empty());
    assert!(b.get_lock("cas-personal").unwrap().is_none());
    assert!(b.renew("cas-personal", 600).is_err());
    b.release("cas-personal").unwrap();
    assert!(b.claim("cas-personal", 600, None).unwrap().is_success());
    assert_eq!(a.get_lock("cas-personal").unwrap().unwrap().agent_id, "a");
}
