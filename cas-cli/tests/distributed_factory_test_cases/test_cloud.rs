//! cas-c7c7: the cloud a distributed test runs against: the in-process fake
//! by default, or the real Cassy Cloud when a token is supplied.
//!
//! Real mode is enabled by `CAS_TEST_CLOUD_TOKEN_FILE`, a 0600 file holding
//! either a `cloud.json` (`{"endpoint", "token"}`) or a bare token.
//! `CAS_TEST_CLOUD_ENDPOINT` overrides the endpoint, which otherwise comes
//! from the file or defaults to production. The `CAS_TEST_CLOUD_` prefix
//! survives `TestEnvGuard`'s scrub of ambient `CAS_*` state, so a guarded test
//! never silently falls back to the fake; the product reads none of these
//! variables.
//!
//! In real mode a local proxy stands in front of the cloud, so every test
//! runs unchanged on real HTTP:
//! - **Namespacing:** it scopes the test's identifiers to one run, so runs
//!   never collide with each other or with real work. Agent ids become
//!   `cc-<run>-<id>`, canonical ids become
//!   `github.com/petrastella-live-check/r<run>-<repo>`, claim keys become
//!   `<key>.r<run>`, and consumer ids and dedupe keys are prefixed. Answers
//!   are mapped back.
//! - **The token:** it holds the token and adds the bearer header, so the
//!   test's scratch `.cas` directories hold a placeholder, never the
//!   credential. The token is never printed or recorded.
//! - **Transcripts:** it records every exchange, ids in the test's own
//!   terms, into `CAS_TEST_CLOUD_TRANSCRIPT_DIR` when that is set.
//! - **Teardown:** on drop it releases every claim it saw, acks every
//!   message it saw leased, rejects anything still queued for the run's
//!   agents, and shuts every agent down. It then verifies the result and
//!   records the check with the transcript.
//!
//! The fake-only hooks have real equivalents:
//! - `age_heartbeat` waits until the cloud itself reports the agent `stale`.
//! - `messages_to` reads the run's sends and their live status.
//! - `set_metadata` heartbeats the metadata.
//! - `set_account` (another user's agents) cannot be done with one account,
//!   so it returns `false` and the transcript records the skip.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::fake_cloud::FakeCloud;

const PRODUCTION: &str = "https://petra-stella-cloud.vercel.app";
/// The credential the test's own config files carry in real mode.
const PLACEHOLDER_TOKEN: &str = "proxy-supplies-the-token";

pub(super) enum TestCloud {
    Fake(FakeCloud),
    Real(RealCloud),
}

impl TestCloud {
    /// The real cloud when a token is supplied, else the fake.
    pub fn start() -> Self {
        match real_credentials() {
            Some((endpoint, token)) => TestCloud::Real(RealCloud::start(endpoint, token)),
            None => TestCloud::Fake(FakeCloud::start()),
        }
    }

    pub fn is_fake(&self) -> bool {
        matches!(self, TestCloud::Fake(_))
    }

    /// A cloud config for this cloud.
    pub fn config(&self) -> cas::cloud::CloudConfig {
        match self {
            TestCloud::Fake(fake) => fake.config(),
            TestCloud::Real(real) => cas::cloud::CloudConfig {
                endpoint: real.url.clone(),
                token: Some(PLACEHOLDER_TOKEN.to_string()),
                ..Default::default()
            },
        }
    }

    /// Point a machine's project at this cloud under `canonical_id`.
    pub fn seed(&self, cas_dir: &std::path::Path, canonical_id: &str) {
        match self {
            TestCloud::Fake(fake) => fake.seed(cas_dir, canonical_id),
            TestCloud::Real(_) => {
                self.config().save_to_cas_dir(cas_dir).unwrap();
                std::fs::write(
                    cas_dir.join("config.toml"),
                    format!("[project]\ncanonical_id = \"{canonical_id}\"\n"),
                )
                .unwrap();
            }
        }
    }

    /// Make `agent_id` read stale: the fake ages its heartbeat; the real
    /// cloud is waited on until it reports the agent stale itself.
    pub fn age_heartbeat(&self, agent_id: &str, minutes: i64) {
        match self {
            TestCloud::Fake(fake) => fake.age_heartbeat(agent_id, minutes),
            TestCloud::Real(real) => real.wait_until_stale(agent_id),
        }
    }

    /// The peer messages addressed to `recipient`: `id` and `status` at
    /// least, plus the sent fields.
    pub fn messages_to(&self, recipient: &str) -> Vec<Value> {
        match self {
            TestCloud::Fake(fake) => fake.messages_to(recipient),
            TestCloud::Real(real) => real.messages_to(recipient),
        }
    }

    /// Set a metadata key on a registered agent.
    pub fn set_metadata(&self, agent_id: &str, key: &str, value: &str) {
        match self {
            TestCloud::Fake(fake) => {
                fake.state.lock().unwrap().agents.get_mut(agent_id).unwrap()["metadata"][key] =
                    json!(value);
            }
            TestCloud::Real(real) => real.heartbeat_metadata(agent_id, key, value),
        }
    }

    /// Re-own `agent_id` as another user's agent on a team project. Only the
    /// fake can: returns `false` on the real cloud (one account), and the
    /// transcript records that the dependent checks were skipped.
    pub fn set_account(&self, agent_id: &str, user_id: &str, team_id: &str) -> bool {
        match self {
            TestCloud::Fake(fake) => {
                let mut state = fake.state.lock().unwrap();
                let agent = state.agents.get_mut(agent_id).unwrap();
                agent["user_id"] = json!(user_id);
                agent["team_id"] = json!(team_id);
                true
            }
            TestCloud::Real(real) => {
                real.note(&format!(
                    "skipped: {agent_id} cannot belong to another account on a one-account run"
                ));
                false
            }
        }
    }
}

/// `(endpoint, token)` when a real-cloud run was requested.
fn real_credentials() -> Option<(String, String)> {
    let mut endpoint = None;
    let path = std::env::var("CAS_TEST_CLOUD_TOKEN_FILE").ok().filter(|p| !p.is_empty())?;
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("CAS_TEST_CLOUD_TOKEN_FILE {path}: {error}"));
    let token = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(config)) => {
            endpoint = config.get("endpoint").and_then(Value::as_str).map(str::to_string);
            config.get("token").and_then(Value::as_str).map(str::to_string)
        }
        _ => Some(text.trim().to_string()),
    }
    .filter(|token| !token.is_empty())
    .unwrap_or_else(|| panic!("CAS_TEST_CLOUD_TOKEN_FILE {path} holds no token"));
    let endpoint = std::env::var("CAS_TEST_CLOUD_ENDPOINT")
        .ok()
        .filter(|e| !e.is_empty())
        .or(endpoint)
        .unwrap_or_else(|| PRODUCTION.to_string());
    Some((endpoint.trim_end_matches('/').to_string(), token))
}

/// What the proxy knows about one run.
struct Run {
    endpoint: String,
    token: String,
    id: String,
    /// Mapped canonical id -> the original (normalised) canonical id.
    projects: BTreeMap<String, String>,
    /// Mapped agent id -> its mapped canonical id, from registration.
    agent_projects: BTreeMap<String, String>,
    agents: BTreeSet<String>,
    /// (mapped claim key, mapped agent id) of every claim attempt.
    claims: BTreeSet<(String, String)>,
    /// (original recipient, message id, original send body) of every send.
    sends: Vec<(String, String, Value)>,
    /// (message id, mapped consumer) of every message leased to the run.
    leases: Vec<(String, String)>,
    /// Requests the cloud rate-limited (HTTP 429) and the proxy retried.
    rate_limited: usize,
    transcript: Vec<Value>,
}

impl Run {
    fn agent(&self, id: &str) -> String {
        let prefix = format!("cc-{}-", self.id);
        if id.starts_with(&prefix) { id.to_string() } else { format!("{prefix}{id}") }
    }

    fn project(&mut self, canonical: &str) -> String {
        if self.projects.contains_key(canonical) {
            return canonical.to_string();
        }
        let normalized = cas::cloud::normalize_project_canonical_id(canonical)
            .unwrap_or_else(|| canonical.to_string());
        let slug: String = normalized
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let mapped = format!("github.com/petrastella-live-check/r{}-{slug}", self.id);
        self.projects.insert(mapped.clone(), normalized);
        mapped
    }

    fn key(&self, key: &str) -> String {
        format!("{key}.r{}", self.id)
    }

    /// Rewrite the request body's identifiers into the run's namespace.
    fn map_body(&mut self, value: &mut Value, register: bool) {
        let Some(map) = value.as_object_mut() else { return };
        let keys: Vec<String> = map.keys().cloned().collect();
        for key in keys {
            let entry = map.get_mut(&key).unwrap();
            match key.as_str() {
                "id" if register => {
                    if let Some(id) = entry.as_str() {
                        *entry = json!(self.agent(id));
                    }
                }
                "agent_id" | "parent_id" | "sender_agent_id" | "recipient_agent_id" | "owner_agent_id" => {
                    if let Some(id) = entry.as_str() {
                        *entry = json!(self.agent(id));
                    }
                }
                "recipient_agent_ids" => {
                    if let Some(ids) = entry.as_array_mut() {
                        for id in ids.iter_mut() {
                            if let Some(text) = id.as_str() {
                                *id = json!(self.agent(text));
                            }
                        }
                    }
                }
                "project_id" | "canonical_id" => {
                    if let Some(project) = entry.as_str().map(str::to_string) {
                        *entry = json!(self.project(&project));
                    }
                }
                "project_ids" => {
                    if let Some(ids) = entry.as_array_mut() {
                        for id in ids.iter_mut() {
                            if let Some(text) = id.as_str().map(str::to_string) {
                                *id = json!(self.project(&text));
                            }
                        }
                    }
                }
                "consumer_id" | "dedupe_key" => {
                    if let Some(text) = entry.as_str() {
                        *entry = json!(format!("cc-{}-{text}", self.id));
                    }
                }
                "metadata" => self.map_body(entry, false),
                _ => {}
            }
        }
    }

    /// Map an answer back into the test's own terms.
    fn unmap(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => {
                let prefix = format!("cc-{}-", self.id);
                let suffix = format!(".r{}", self.id);
                if let Some(original) = self.projects.get(text) {
                    json!(original)
                } else if let Some(rest) = text.strip_prefix(&prefix) {
                    json!(rest)
                } else if let Some(rest) = text.strip_suffix(&suffix) {
                    json!(rest)
                } else {
                    value.clone()
                }
            }
            Value::Array(items) => Value::Array(items.iter().map(|v| self.unmap(v)).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), self.unmap(v))).collect()),
            other => other.clone(),
        }
    }
}

pub(super) struct RealCloud {
    pub url: String,
    run: Arc<Mutex<Run>>,
    server: Arc<tiny_http::Server>,
    test: String,
}

impl RealCloud {
    fn start(endpoint: String, token: String) -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("proxy binds"));
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let id = format!("{:08x}", rand_u32());
        let run = Arc::new(Mutex::new(Run {
            endpoint,
            token,
            id,
            projects: BTreeMap::new(),
            agent_projects: BTreeMap::new(),
            agents: BTreeSet::new(),
            claims: BTreeSet::new(),
            sends: Vec::new(),
            leases: Vec::new(),
            rate_limited: 0,
            transcript: Vec::new(),
        }));
        let (thread_server, thread_run) = (Arc::clone(&server), Arc::clone(&run));
        std::thread::spawn(move || {
            for mut request in thread_server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let (status, reply) =
                    forward(&thread_run, request.method().as_str(), request.url(), &body);
                let response = tiny_http::Response::from_string(reply.to_string())
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                            .unwrap(),
                    );
                let _ = request.respond(response);
            }
        });
        let test = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .replace("::", "-");
        Self { url, run, server, test }
    }

    fn note(&self, note: &str) {
        self.run.lock().unwrap().transcript.push(json!({"note": note}));
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = ureq::request(method, &format!("{}{path}", self.url))
            .set("Authorization", &format!("Bearer {PLACEHOLDER_TOKEN}"))
            .timeout(std::time::Duration::from_secs(20));
        let result = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        };
        match result {
            Ok(response) => (response.status(), response.into_json().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(code, response)) => (code, response.into_json().unwrap_or(Value::Null)),
            Err(error) => panic!("proxy call {method} {path}: {error}"),
        }
    }

    fn wait_until_stale(&self, agent_id: &str) {
        let project = {
            let run = self.run.lock().unwrap();
            let mapped = run.agent(agent_id);
            run.agent_projects.get(&mapped).and_then(|p| run.projects.get(p)).cloned()
        };
        let query = project
            .map(|p| format!("?project={}&status=stale", urlencode(&p)))
            .unwrap_or_else(|| "?status=stale&limit=500".to_string());
        self.note(&format!("waiting for the cloud to report {agent_id} stale (five minutes)"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(420);
        loop {
            let (_, body) = self.call("GET", &format!("/api/agents{query}"), None);
            let stale = body["agents"]
                .as_array()
                .is_some_and(|agents| agents.iter().any(|a| a["id"] == agent_id));
            if stale {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{agent_id} never read stale");
            std::thread::sleep(std::time::Duration::from_secs(15));
        }
    }

    fn messages_to(&self, recipient: &str) -> Vec<Value> {
        let sends: Vec<(String, Value)> = self
            .run
            .lock()
            .unwrap()
            .sends
            .iter()
            .filter(|(to, _, _)| to == recipient)
            .map(|(_, id, body)| (id.clone(), body.clone()))
            .collect();
        sends
            .into_iter()
            .map(|(id, mut sent)| {
                let (_, status) = self.call("GET", &format!("/api/peer-messages/{id}"), None);
                sent["id"] = json!(id);
                sent["status"] = status["status"].clone();
                sent["attempts"] = status["attempts"].clone();
                sent
            })
            .collect()
    }

    fn heartbeat_metadata(&self, agent_id: &str, key: &str, value: &str) {
        let (status, body) = self.call(
            "POST",
            &format!("/api/agents/{agent_id}/heartbeat"),
            Some(json!({"metadata": {key: value}})),
        );
        assert_eq!(status, 200, "heartbeat {agent_id}: {body}");
    }

    /// Release, ack, reject, shut down; then verify. Returns the checks.
    fn teardown(&self) -> Value {
        let (claims, leases, agents, projects, id) = {
            let run = self.run.lock().unwrap();
            (
                run.claims.clone(),
                run.leases.clone(),
                run.agents.clone(),
                run.projects.clone(),
                run.id.clone(),
            )
        };
        let prefix = format!("cc-{id}-");
        let suffix = format!(".r{id}");
        let local = |mapped: &str| mapped.strip_prefix(&prefix).unwrap_or(mapped).to_string();
        for (key, agent) in &claims {
            let key = key.strip_suffix(&suffix).unwrap_or(key);
            self.call("POST", &format!("/api/agents/tasks/{key}/release"), Some(json!({"agent_id": local(agent)})));
        }
        for (message, consumer) in &leases {
            self.call(
                "POST",
                "/api/peer-messages/ack",
                Some(json!({"consumer_id": local(consumer), "acks": [{"id": message, "outcome": "rejected"}]})),
            );
        }
        let recipients: Vec<String> = agents.iter().map(|a| local(a)).collect();
        let originals: Vec<String> = projects.values().cloned().collect();
        if !recipients.is_empty() && !originals.is_empty() {
            for chunk in recipients.chunks(50) {
                let (_, left) = self.call(
                    "POST",
                    "/api/peer-messages/claim",
                    Some(json!({"project_ids": originals, "recipient_agent_ids": chunk,
                                "consumer_id": "teardown", "max": 50, "lease_secs": 30})),
                );
                let ids: Vec<Value> = left["messages"]
                    .as_array()
                    .map(|m| m.iter().map(|m| json!({"id": m["id"], "outcome": "rejected"})).collect())
                    .unwrap_or_default();
                if !ids.is_empty() {
                    self.call("POST", "/api/peer-messages/ack", Some(json!({"consumer_id": "teardown", "acks": ids})));
                }
            }
        }
        for agent in &recipients {
            self.call("POST", &format!("/api/agents/{agent}/shutdown"), None);
        }
        // Verify against the cloud directly, in mapped terms.
        let (_, locks) = self.call("GET", "/api/agents/locks", None);
        let leftover_locks = locks["locks"]
            .as_array()
            .map(|locks| {
                locks
                    .iter()
                    .filter(|lock| claims.iter().any(|(_, a)| lock["agent_id"] == local(a).as_str()))
                    .count()
            })
            .unwrap_or(usize::MAX);
        // Every agent of the run, with or without a project: the account
        // listing, which the proxy scopes to this run, followed to its end.
        let mut live_agents = Vec::new();
        let mut query = "limit=500".to_string();
        for _ in 0..40 {
            let (_, page) = self.call("GET", &format!("/api/agents?{query}"), None);
            for agent in page["agents"].as_array().cloned().unwrap_or_default() {
                if agent["status"] != "shutdown" && recipients.iter().any(|r| agent["id"] == r.as_str()) {
                    live_agents.push(agent["id"].clone());
                }
            }
            match (page["next_cursor"].as_str(), page["next_cursor_id"].as_str()) {
                (Some(at), Some(id)) => {
                    query = format!("limit=500&cursor={}&cursor_id={}", urlencode(at), urlencode(id));
                    if let Some(sort) = page["next_sort_cursor"].as_str() {
                        query.push_str(&format!("&sort_cursor={}", urlencode(sort)));
                    }
                }
                _ => break,
            }
        }
        let rate_limited = self.run.lock().unwrap().rate_limited;
        json!({
            "claims_released": leftover_locks == 0,
            "agents_shutdown": live_agents.is_empty(),
            "live_agents": live_agents,
            "agents": recipients.len(),
            "claims": claims.len(),
            "leases_acked": leases.len(),
            "rate_limited_retries": rate_limited,
        })
    }
}

impl Drop for RealCloud {
    fn drop(&mut self) {
        let checks = self.teardown();
        self.server.unblock();
        let run = self.run.lock().unwrap();
        if let Ok(dir) = std::env::var("CAS_TEST_CLOUD_TRANSCRIPT_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            let document = json!({
                "test": self.test,
                "run": run.id,
                "endpoint": run.endpoint,
                "exchanges": run.transcript,
                "teardown": checks,
            });
            let path = dir.join(format!("{}-{}.json", self.test, run.id));
            let text = serde_json::to_string_pretty(&document).unwrap();
            assert!(!text.contains(&run.token), "the transcript must never hold the token");
            let _ = std::fs::write(path, text);
        }
        if !std::thread::panicking() {
            assert_eq!(checks["claims_released"], true, "teardown left claims: {checks}");
            assert_eq!(checks["agents_shutdown"], true, "teardown left live agents: {checks}");
        }
    }
}

fn rand_u32() -> u32 {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Forward one request to the real cloud in the run's namespace and map the
/// answer back, recording both.
fn forward(run: &Mutex<Run>, method: &str, url: &str, raw: &str) -> (u16, Value) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let mut run = run.lock().unwrap();
    let segments: Vec<String> = path.trim_start_matches('/').split('/').map(str::to_string).collect();
    let mut mapped_segments = segments.clone();
    let mut claim: Option<String> = None;
    match segments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["api", "agents", "tasks", key, _] => {
            mapped_segments[3] = run.key(key);
            claim = Some(mapped_segments[3].clone());
        }
        ["api", "agents", id, "heartbeat" | "shutdown" | "locks"] if *id != "tasks" => {
            mapped_segments[2] = run.agent(id);
        }
        _ => {}
    }
    let mut pairs = url::form_urlencoded::Serializer::new(String::new());
    let original_query: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    for (key, value) in &original_query {
        let value = match key.as_str() {
            "project" => run.project(value),
            "cursor_id" => run.agent(value),
            _ => value.clone(),
        };
        pairs.append_pair(key, &value);
    }
    let mapped_query = pairs.finish();
    let original_body: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let register = path == "/api/agents/register";
    let mut body = original_body.clone();
    run.map_body(&mut body, register);
    if register
        && let (Some(id), Some(project)) = (body["id"].as_str(), body["metadata"]["canonical_id"].as_str())
    {
        run.agent_projects.insert(id.to_string(), project.to_string());
    }
    for field in ["id", "agent_id"] {
        if let Some(agent) = body[field].as_str().filter(|_| register || field == "agent_id") {
            run.agents.insert(agent.to_string());
        }
    }
    if let (Some(key), Some(agent)) = (&claim, body["agent_id"].as_str()) {
        run.claims.insert((key.clone(), agent.to_string()));
    }
    let target = format!(
        "{}/{}{}",
        run.endpoint,
        mapped_segments.join("/"),
        if mapped_query.is_empty() { String::new() } else { format!("?{mapped_query}") }
    );
    // Many tests share one account, and production rate-limits an account
    // (429). A limited request is retried with backoff, and every retry is
    // recorded, so the limit stays visible without failing a test or a
    // teardown on load alone.
    let mut retries = 0;
    let (status, text) = loop {
        let request = ureq::request(method, &target)
            .set("Authorization", &format!("Bearer {}", run.token))
            .timeout(std::time::Duration::from_secs(20));
        let result = if raw.is_empty() {
            request.call()
        } else {
            request.set("Content-Type", "application/json").send_json(body.clone())
        };
        let (status, text, retry_after) = match result {
            Ok(response) => (response.status(), response.into_string().unwrap_or_default(), None),
            Err(ureq::Error::Status(code, response)) => {
                let retry_after = response.header("Retry-After").and_then(|s| s.trim().parse::<u64>().ok());
                (code, response.into_string().unwrap_or_default(), retry_after)
            }
            Err(error) => (599, json!({"error": format!("transport: {error}")}).to_string(), None),
        };
        if status != 429 || retries >= 6 {
            break (status, text);
        }
        retries += 1;
        let wait = retry_after.unwrap_or(1 << retries.min(3)).min(15);
        std::thread::sleep(std::time::Duration::from_secs(wait));
    };
    run.rate_limited += retries;
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    // Track what teardown must undo.
    if path == "/api/peer-messages" && status == 200
        && let (Some(id), Some(to)) = (answer["id"].as_str(), original_body["recipient_agent_id"].as_str())
        && answer["status"] == "queued"
    {
        run.sends.push((to.to_string(), id.to_string(), original_body.clone()));
    }
    if path == "/api/peer-messages/claim" && status == 200
        && let Some(consumer) = body["consumer_id"].as_str()
    {
        for message in answer["messages"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = message["id"].as_str() {
                run.leases.push((id.to_string(), consumer.to_string()));
            }
        }
    }
    // An account-wide listing shows only this run: parallel tests share one
    // account, and one test's agents or claims are never another's.
    let mut answer = answer;
    let prefix = format!("cc-{}-", run.id);
    let suffix = format!(".r{}", run.id);
    let account_wide = method == "GET"
        && (path == "/api/agents/locks"
            || (path == "/api/agents" && !original_query.iter().any(|(key, _)| key == "project")));
    if account_wide {
        for list in ["locks", "agents"] {
            if let Some(items) = answer.get_mut(list).and_then(Value::as_array_mut) {
                items.retain(|item| {
                    item["task_id"].as_str().is_some_and(|key| key.ends_with(&suffix))
                        || (list == "agents" && item["id"].as_str().is_some_and(|id| id.starts_with(&prefix)))
                });
            }
        }
    }
    let unmapped = run.unmap(&answer);
    run.transcript.push(json!({
        "method": method,
        "path": path,
        "query": original_query.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "request": original_body,
        "forwarded": format!("/{}", mapped_segments.join("/")),
        "status": status,
        "rate_limited_retries": retries,
        "response": unmapped,
    }));
    (status, unmapped)
}

/// The real-cloud proxy, pointed at the in-process fake: the cloud only ever
/// sees run-scoped identifiers, the test only ever sees its own, and
/// teardown leaves nothing claimed or live.
#[test]
fn the_real_cloud_proxy_namespaces_ids_and_tears_down() {
    use cas::cloud::CloudCoordinator;
    use cas::types::{Agent, ClaimResult};
    let fake = FakeCloud::start();
    let cloud = TestCloud::Real(RealCloud::start(fake.url.clone(), "test-token".into()));
    let config = cloud.config();
    assert_ne!(config.token.as_deref(), Some("test-token"), "the test never holds the token");

    let mut alpha = CloudCoordinator::new(config.clone())
        .unwrap()
        .with_canonical_id(Some("github.com/acme/widgets".into()));
    alpha.register(&Agent::new("a".into(), "sup-a".into())).unwrap();
    let mut beta = CloudCoordinator::new(config.clone())
        .unwrap()
        .with_canonical_id(Some("git@github.com:acme/widgets.git".into()));
    beta.register(&Agent::new("b".into(), "sup-b".into())).unwrap();
    {
        let state = fake.state.lock().unwrap();
        assert!(state.agents.keys().all(|id| id.starts_with("cc-")), "{:?}", state.agents.keys());
        let projects: BTreeSet<&str> = state
            .agents
            .values()
            .filter_map(|agent| agent["metadata"]["canonical_id"].as_str())
            .collect();
        assert_eq!(projects.len(), 1, "both spellings map to one run project: {projects:?}");
        assert!(projects.iter().all(|p| p.starts_with("github.com/petrastella-live-check/r")));
    }

    let listed = CloudCoordinator::new(config.clone()).unwrap().list_agent_infos().unwrap();
    let mut ids: Vec<&str> = listed.iter().map(|agent| agent.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(listed[0].metadata["canonical_id"], "github.com/acme/widgets");

    match alpha.claim("cas-t1", 600, None).unwrap() {
        ClaimResult::Success(lock) => assert_eq!(lock.task_id, "cas-t1"),
        other => panic!("{other:?}"),
    }
    match beta.claim("cas-t1", 600, None).unwrap() {
        ClaimResult::AlreadyClaimed { held_by, .. } => assert_eq!(held_by, "a"),
        other => panic!("{other:?}"),
    }
    assert!(fake.state.lock().unwrap().locks.keys().all(|key| key.starts_with("cas-t1.r")));

    drop(cloud);
    let state = fake.state.lock().unwrap();
    assert!(state.agents.values().all(|agent| agent["_shutdown"] == true), "every agent shut down");
    assert!(state.locks.values().all(|lock| lock["status"] != "active"), "every claim released");
}

/// Two runs share one account (parallel tests on one token): neither sees
/// the other's agents or claims in an account-wide listing, so a test's view
/// of "the account" is its own run (cas-c7c7; the first real-cloud leg failed
/// when one test listed another test's lock).
#[test]
fn concurrent_runs_on_one_account_never_see_each_other() {
    use cas::cloud::CloudCoordinator;
    use cas::types::Agent;
    let fake = FakeCloud::start();
    let first = TestCloud::Real(RealCloud::start(fake.url.clone(), "test-token".into()));
    let second = TestCloud::Real(RealCloud::start(fake.url.clone(), "test-token".into()));
    let mut holder = CloudCoordinator::new(first.config()).unwrap();
    holder.register(&Agent::new("a".into(), "sup-a".into())).unwrap();
    holder.claim("cas-t1", 600, None).unwrap();
    let mut other = CloudCoordinator::new(second.config()).unwrap();
    other.register(&Agent::new("b".into(), "sup-b".into())).unwrap();

    assert_eq!(other.list_locks().unwrap().len(), 0, "the first run's claim is not the second run's");
    assert_eq!(holder.list_locks().unwrap().len(), 1, "a run still sees its own claim");
    let theirs: Vec<String> = other.list_agent_infos().unwrap().into_iter().map(|agent| agent.id).collect();
    assert_eq!(theirs, ["b"], "the second run lists only its own agents");
}

/// Production rate-limits an account (HTTP 429) when many tests share it.
/// The proxy retries a limited request, so teardown still shuts every agent
/// down (the second real-cloud leg left live agents behind exactly this way),
/// and it records the retries so the limit stays visible.
#[test]
fn a_rate_limited_teardown_still_shuts_every_agent_down() {
    use cas::cloud::CloudCoordinator;
    use cas::types::Agent;
    let fake = FakeCloud::start();
    let cloud = TestCloud::Real(RealCloud::start(fake.url.clone(), "test-token".into()));
    let mut coordinator = CloudCoordinator::new(cloud.config()).unwrap();
    coordinator.register(&Agent::new("a".into(), "sup-a".into())).unwrap();
    coordinator.register(&Agent::new("b".into(), "sup-b".into())).unwrap();
    // The fake answers these 429s with Retry-After: 0.
    fake.inject_failure("POST", "/shutdown", 429, 2);
    drop(cloud);
    let state = fake.state.lock().unwrap();
    assert!(state.injected.iter().all(|(.., left)| *left == 0), "both 429s were retried");
    assert!(state.agents.values().all(|agent| agent["_shutdown"] == true), "every agent shut down");
}

/// An account-wide listing keeps exactly the keys production sent: filtering
/// to the run never adds a key.
#[test]
fn run_scoped_listings_keep_productions_keys() {
    let fake = FakeCloud::start();
    let real = RealCloud::start(fake.url.clone(), "test-token".into());
    let (_, locks) = real.call("GET", "/api/agents/locks", None);
    let keys: BTreeSet<&str> = locks.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["status", "locks"]), "{locks}");
}
