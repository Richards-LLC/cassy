//! cas-c7c7: replay the recorded production cloud contract against a fake.
//!
//! `tests/fixtures/cloud_contract/*.json` holds request/response pairs
//! captured from production by `scripts/cloud-contract-capture.py`
//! (cas-2911), normalised to placeholders: `<agent-a>`, `<project>`,
//! `<uuid-N>`, `<ts-N>`, and so on. [`Replay`] sends each recorded request,
//! with placeholders bound to live values, to a fake cloud. It then asserts
//! that the fake answered with the recorded status and the same JSON: the
//! same keys at every level and the same literals. A placeholder the fixture
//! saw before must come back with the value it was bound to, so relations
//! hold across exchanges (a re-claim keeps the first claim's `expires_at`, a
//! duplicate send returns the original id). Exchanges whose payload depends
//! on the whole production account are compared on shape only.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Map, Value};

/// Exchanges whose payload lists everything in the production account, so
/// only their shape can match a fresh fake.
pub const SHAPE_ONLY: &[&str] = &["list_without_project", "list_locks"];

/// One recorded request and the response production gave.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub group: String,
    pub name: String,
    pub method: String,
    pub path: String,
    pub query: Option<Map<String, Value>>,
    pub body: Option<Value>,
    pub status: u16,
    pub response: Value,
}

/// The recorded exchanges of `group` (`agents`, `claims`, `peer_messages`,
/// `teardown`), in capture order, without the computed teardown summary.
pub fn fixtures(group: &str) -> Vec<Exchange> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cloud_contract")
        .join(format!("{group}.json"));
    let document: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    document["exchanges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|exchange| exchange["request"].is_object())
        .map(|exchange| {
            let request = &exchange["request"];
            Exchange {
                group: group.to_string(),
                name: exchange["name"].as_str().unwrap().to_string(),
                method: request["method"].as_str().unwrap().to_string(),
                path: request["path"].as_str().unwrap().to_string(),
                query: request["query"].as_object().cloned(),
                body: Some(request["body"].clone()).filter(|body| !body.is_null()),
                status: exchange["response"]["status"].as_u64().unwrap() as u16,
                response: exchange["response"]["body"].clone(),
            }
        })
        .collect()
}

/// Placeholder tokens in `text`: `<name>` where name is `[a-z0-9-]+`.
fn placeholders(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let tail = &rest[start + 1..];
        match tail.find('>') {
            Some(end)
                if end > 0
                    && tail[..end]
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') =>
            {
                found.push(format!("<{}>", &tail[..end]));
                rest = &tail[end + 1..];
            }
            _ => rest = tail,
        }
    }
    found
}

/// Sends recorded requests to a cloud and compares its answers with the
/// recording.
pub struct Replay {
    base: String,
    bindings: HashMap<String, String>,
    pub failures: Vec<String>,
}

impl Replay {
    /// A replay against `base` (the fake's URL). `run` scopes the scratch
    /// names the fixtures use, so placeholders like `cc-<run>-stranger`
    /// become `cc-<run id>-stranger`.
    pub fn new(base: &str, run: &str) -> Self {
        let project = format!("github.com/petrastella-live-check/qa-{run}");
        let mut bindings = HashMap::new();
        for (placeholder, value) in [
            ("<agent-a>", format!("cc-{run}-a")),
            ("<agent-b>", format!("cc-{run}-b")),
            ("<agent-c>", format!("cc-{run}-c")),
            ("<factory-session>", format!("cc-{run}-session")),
            ("<consumer>", format!("cc-{run}-consumer")),
            ("<host>", "parity-host".to_string()),
            ("<run>", run.to_string()),
            (
                "<task-key-1>",
                cas::cloud::peer_claims::claim_key(&project, &format!("cas-{run}t1")),
            ),
            (
                "<task-key-2>",
                cas::cloud::peer_claims::claim_key(&project, &format!("cas-{run}t2")),
            ),
            ("<project>", project.clone()),
        ] {
            bindings.insert(placeholder.to_string(), value);
        }
        Self { base: base.trim_end_matches('/').to_string(), bindings, failures: Vec::new() }
    }

    fn fail(&mut self, exchange: &Exchange, at: &str, what: String) {
        self.failures.push(format!("{}/{} {at}: {what}", exchange.group, exchange.name));
    }

    /// `text` with every bound placeholder replaced. An unbound `<uuid-N>`
    /// in a request is an id production saw nowhere else (an unknown id),
    /// so it is bound to a fresh UUID.
    fn substitute(&mut self, text: &str) -> String {
        let mut out = text.to_string();
        for token in placeholders(text) {
            if !self.bindings.contains_key(&token) && token.starts_with("<uuid-") {
                self.bindings.insert(token.clone(), uuid::Uuid::new_v4().to_string());
            }
            if let Some(value) = self.bindings.get(&token) {
                out = out.replace(&token, value);
            }
        }
        out
    }

    fn substitute_value(&mut self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.substitute(text)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.substitute_value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter().map(|(k, v)| (k.clone(), self.substitute_value(v))).collect(),
            ),
            other => other.clone(),
        }
    }

    /// Send `exchange`'s request; returns the status and the parsed body.
    pub fn send(&mut self, exchange: &Exchange) -> (u16, Value) {
        let path = self.substitute(&exchange.path);
        let mut url = format!("{}{path}", self.base);
        if let Some(query) = &exchange.query {
            let mut pairs = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in query {
                let value = match value {
                    Value::String(text) => self.substitute(text),
                    other => other.to_string(),
                };
                pairs.append_pair(key, &value);
            }
            url = format!("{url}?{}", pairs.finish());
        }
        let request = ureq::request(&exchange.method, &url)
            .set("Authorization", "Bearer test-token")
            .timeout(std::time::Duration::from_secs(10));
        let response = match exchange.body.as_ref().map(|body| self.substitute_value(body)) {
            Some(body) => request.send_json(body),
            None => request.call(),
        };
        let (status, text) = match response {
            Ok(response) => (response.status(), response.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(code, response)) => (code, response.into_string().unwrap_or_default()),
            Err(error) => panic!("{}/{}: transport error: {error}", exchange.group, exchange.name),
        };
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// Send `exchange` and record every way the answer differs from the
    /// recording.
    pub fn check(&mut self, exchange: &Exchange) {
        let (status, body) = self.send(exchange);
        if status != exchange.status {
            self.fail(exchange, "status", format!("expected HTTP {}, got {status}: {body}", exchange.status));
            return;
        }
        let shape_only = SHAPE_ONLY.contains(&exchange.name.as_str());
        self.compare(exchange, "$", &exchange.response.clone(), &body, shape_only);
    }

    fn compare(&mut self, exchange: &Exchange, at: &str, recorded: &Value, live: &Value, shape_only: bool) {
        match (recorded, live) {
            (Value::Object(want), Value::Object(got)) => {
                let missing: Vec<_> = want.keys().filter(|k| !got.contains_key(*k)).cloned().collect();
                let extra: Vec<_> = got.keys().filter(|k| !want.contains_key(*k)).cloned().collect();
                if !missing.is_empty() || !extra.is_empty() {
                    self.fail(exchange, at, format!("keys differ: missing {missing:?}, extra {extra:?}"));
                }
                for (key, value) in want {
                    if let Some(other) = got.get(key) {
                        self.compare(exchange, &format!("{at}.{key}"), value, other, shape_only);
                    }
                }
            }
            (Value::Array(want), Value::Array(got)) => {
                if shape_only {
                    if let (Some(first), Some(other)) = (want.first(), got.first()) {
                        self.compare(exchange, &format!("{at}[0]"), first, other, true);
                    }
                    return;
                }
                if want.len() != got.len() {
                    self.fail(exchange, at, format!("expected {} items, got {}: {live}", want.len(), got.len()));
                    return;
                }
                for (index, (value, other)) in want.iter().zip(got).enumerate() {
                    self.compare(exchange, &format!("{at}[{index}]"), value, other, false);
                }
            }
            (Value::String(want), Value::String(got)) => {
                if shape_only {
                    return;
                }
                let tokens = placeholders(want);
                // A whole-value placeholder seen for the first time binds to
                // what the fake returned; afterwards it must come back equal.
                if tokens.len() == 1 && tokens[0] == *want && !self.bindings.contains_key(want) {
                    self.bindings.insert(want.clone(), got.clone());
                    return;
                }
                let expected = self.substitute(want);
                if expected != *got {
                    self.fail(exchange, at, format!("expected {expected:?}, got {got:?}"));
                }
            }
            (want, got) if std::mem::discriminant(want) == std::mem::discriminant(got) => {
                if !shape_only && want != got {
                    self.fail(exchange, at, format!("expected {want}, got {got}"));
                }
            }
            (want, got) => self.fail(exchange, at, format!("type differs: expected {want}, got {got}")),
        }
    }

    /// Panic with every recorded difference.
    pub fn assert_clean(&self) {
        assert!(
            self.failures.is_empty(),
            "the fake drifted from the production contract ({} differences):\n{}",
            self.failures.len(),
            self.failures.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_found_inside_text() {
        assert_eq!(placeholders("cc-<run>-a <agent-b> a < b <Not> <>"), vec!["<run>", "<agent-b>"]);
    }
}
