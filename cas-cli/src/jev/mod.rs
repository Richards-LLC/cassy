//! Calibrated TypeSafe decisions shared by CLI, MCP and advisory callers.
//! Each evaluation (including unavailable calls) records only a state hash.
use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const DIRECT_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
// Leave margin beneath the MCP server's 55-second response timeout.
const BATCH_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_ATTEMPTS: usize = 3;
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JevConfig {
    pub model: String,
    pub key_file: Option<String>,
    pub enabled: bool,
    pub gate: JevGateConfig,
}
impl Default for JevConfig {
    fn default() -> Self {
        Self {
            model: "jev-1.13.0".into(),
            key_file: None,
            enabled: true,
            gate: JevGateConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JevGateConfig {
    pub shadow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        legend: BTreeMap<String, Value>,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
}

/// Advisory failures are values, allowing callers to retain their normal policy.
/// Success serializes exactly as the TypeSafe response (no envelope).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Outcome {
    Available(Response),
    Unavailable {
        status: UnavailableStatus,
        reason: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnavailableStatus {
    Unavailable,
}

#[derive(Debug)]
pub enum JevError {
    InvalidInput(String),
    Unavailable(String),
    DecisionLog(std::io::Error),
}
impl fmt::Display for JevError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(s) | Self::Unavailable(s) => f.write_str(s),
            Self::DecisionLog(_) => f.write_str("Could not append Jev decision log"),
        }
    }
}
impl std::error::Error for JevError {}

struct Transport {
    endpoint: String,
    token: String,
    cloud: bool,
    team_id: Option<String>,
    project_id: Option<String>,
}
impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("token", &"[REDACTED]")
            .field("cloud", &self.cloud)
            .finish()
    }
}
#[derive(Debug)]
pub struct JevClient {
    config: JevConfig,
    transport: Result<Transport, String>,
    log_path: PathBuf,
    timeout: Duration,
}
impl JevClient {
    /// Resolve credentials without printing or persisting them. Explicit dev
    /// credentials select direct transport; otherwise use the cloud proxy.
    pub fn from_project(cas_root: &Path) -> Result<Self, JevError> {
        let config = crate::config::Config::load(cas_root)
            .map_err(|_| JevError::InvalidInput("Could not load Jev configuration".into()))?
            .jev
            .unwrap_or_default();
        let transport = if !config.enabled {
            Err("Jev is disabled (jev.enabled=false)".into())
        } else {
            resolve_transport(cas_root, &config)
        };
        Ok(Self {
            config,
            transport,
            log_path: cas_root.join("jev-decisions.jsonl"),
            timeout: CALL_TIMEOUT,
        })
    }

    /// Every logical evaluation appends one row, including failures. `advisory`
    /// converts service/config/log unavailability to a typed fail-open value;
    /// malformed input remains an error so callers can correct it.
    pub fn ask(
        &self,
        state: &Value,
        questions: &Value,
        caller: &str,
        advisory: bool,
    ) -> Result<Outcome, JevError> {
        self.ask_until(
            state,
            questions,
            caller,
            advisory,
            Instant::now() + self.timeout,
        )
    }

    /// Ordered batch of 1–50 states. Retries and the entire batch are bounded.
    pub fn batch(
        &self,
        states: &[Value],
        questions: &Value,
        caller: &str,
        advisory: bool,
    ) -> Result<Vec<Outcome>, JevError> {
        self.batch_until(
            states,
            questions,
            caller,
            advisory,
            Instant::now() + BATCH_TIMEOUT,
        )
    }

    fn batch_until(
        &self,
        states: &[Value],
        questions: &Value,
        caller: &str,
        advisory: bool,
        deadline: Instant,
    ) -> Result<Vec<Outcome>, JevError> {
        if !(1..=50).contains(&states.len()) {
            return Err(JevError::InvalidInput(
                "Jev batch requires 1–50 records".into(),
            ));
        }
        for state in states {
            validate_input(state, questions)?;
        }
        // Evaluate/log all records even if strict transport failed on a prior
        // record, then return the first error rather than dropping log rows.
        let results: Vec<_> = states
            .iter()
            .map(|state| {
                self.ask_until(
                    state,
                    questions,
                    caller,
                    advisory,
                    deadline.min(Instant::now() + self.timeout),
                )
            })
            .collect();
        results.into_iter().collect()
    }

    fn ask_until(
        &self,
        state: &Value,
        questions: &Value,
        caller: &str,
        advisory: bool,
        deadline: Instant,
    ) -> Result<Outcome, JevError> {
        let start = Instant::now();
        let mut request_id = None;
        let result = validate_input(state, questions)
            .and_then(|_| self.evaluate(state, questions, deadline, &mut request_id));
        let row = json!({
            "timestamp": Utc::now().to_rfc3339(), "caller": caller,
            "state_hash": format!("{:x}", Sha256::digest(serde_json::to_vec(state).expect("JSON value serializes"))),
            "question_ids": questions.as_object().map(|q| q.keys().collect::<Vec<_>>()).unwrap_or_default(),
            "model": result.as_ref().map(|r| r.model.as_str()).unwrap_or(&self.config.model),
            "answers": result.as_ref().ok().map(|r| &r.answers),
            "input_tokens": result.as_ref().ok().map(|r| r.usage.input_tokens),
            "latency_ms": start.elapsed().as_millis(), "request_id": request_id,
            "status": if result.is_ok() { "available" } else { "unavailable" },
        });
        if let Err(error) = append_log(&self.log_path, &row) {
            return if advisory {
                Ok(unavailable("Jev decision log unavailable"))
            } else {
                Err(JevError::DecisionLog(error))
            };
        }
        match result {
            Ok(response) => Ok(Outcome::Available(response)),
            Err(JevError::Unavailable(reason)) if advisory => Ok(unavailable(&reason)),
            Err(error) => Err(error),
        }
    }

    fn evaluate(
        &self,
        state: &Value,
        questions: &Value,
        deadline: Instant,
        request_id: &mut Option<String>,
    ) -> Result<Response, JevError> {
        if !self.config.enabled {
            return Err(JevError::Unavailable(
                "Jev is disabled (jev.enabled=false)".into(),
            ));
        }
        let transport = self
            .transport
            .as_ref()
            .map_err(|reason| JevError::Unavailable(reason.clone()))?;
        let mut body = json!({"state": state, "questions": questions, "model": self.config.model});
        if transport.cloud {
            if let Some(team) = &transport.team_id {
                body["team_id"] = json!(team);
            }
            if let Some(project) = &transport.project_id {
                body["project_id"] = json!(project);
            }
        }
        // Never forward credentials across redirects; never include server
        // error bodies or URL-bearing network errors in output or logs.
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        for attempt in 0..MAX_ATTEMPTS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(JevError::Unavailable("Jev deadline exceeded".into()));
            }
            let response = agent
                .post(&transport.endpoint)
                .set("Authorization", &format!("Bearer {}", transport.token))
                .timeout(remaining)
                .send_json(&body);
            let response = match response {
                Ok(response) | Err(ureq::Error::Status(_, response)) => response,
                Err(ureq::Error::Transport(_)) => {
                    return Err(JevError::Unavailable(
                        "Jev transport unreachable or timed out".into(),
                    ));
                }
            };
            *request_id = response.header("X-Request-Id").map(str::to_string);
            let status = response.status();
            if status == 200 {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_RESPONSE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| JevError::Unavailable("Could not read Jev response".into()))?;
                if bytes.len() as u64 > MAX_RESPONSE_BYTES {
                    return Err(JevError::Unavailable(
                        "Jev response exceeds size limit".into(),
                    ));
                }
                let response: Response = serde_json::from_slice(&bytes)
                    .map_err(|_| JevError::Unavailable("Invalid Jev response".into()))?;
                validate_response(&response, questions)?;
                return Ok(response);
            }
            if matches!(status, 429 | 529) && attempt + 1 < MAX_ATTEMPTS {
                let delay = response
                    .header("Retry-After")
                    .and_then(retry_after)
                    .unwrap_or(Duration::from_millis(250 * (1 << attempt)));
                if delay >= deadline.saturating_duration_since(Instant::now()) {
                    return Err(JevError::Unavailable(format!(
                        "Jev HTTP {status}; retry exceeds deadline"
                    )));
                }
                std::thread::sleep(delay);
                continue;
            }
            return Err(JevError::Unavailable(format!("Jev HTTP {status}")));
        }
        unreachable!("bounded retry always returns")
    }
}

fn unavailable(reason: &str) -> Outcome {
    Outcome::Unavailable {
        status: UnavailableStatus::Unavailable,
        reason: reason.into(),
    }
}

fn resolve_transport(cas_root: &Path, config: &JevConfig) -> Result<Transport, String> {
    let env_key = std::env::var("TYPESAFE_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty());
    let direct_key = if env_key.is_some() {
        env_key
    } else if let Some(path) = &config.key_file {
        let path = if let Some(rest) = path.strip_prefix("~/") {
            dirs::home_dir()
                .ok_or("Cannot resolve Jev key home directory")?
                .join(rest)
        } else {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                cas_root.join(path)
            }
        };
        Some(fs::read_to_string(path).map_err(|_| "Could not read Jev key file")?)
    } else {
        None
    };
    if let Some(key) = direct_key {
        return Ok(Transport {
            endpoint: DIRECT_ENDPOINT.into(),
            token: valid_token(key)?,
            cloud: false,
            team_id: None,
            project_id: None,
        });
    }
    let mut cloud = crate::cloud::CloudConfig::load_from_cas_dir(cas_root)
        .map_err(|_| "Could not load cloud credentials")?;
    if let Ok(user) = crate::cloud::CloudConfig::load_user() {
        cloud.inherit_credentials_from(&user);
    }
    let endpoint = format!("{}/api/jev", cloud.endpoint.trim_end_matches('/'));
    let url = url::Url::parse(&endpoint).map_err(|_| "Invalid Jev cloud endpoint")?;
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        return Err("Jev cloud endpoint requires HTTPS".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Invalid Jev cloud endpoint".into());
    }
    let team_id = cloud.active_team_id();
    Ok(Transport {
        endpoint,
        token: valid_token(cloud.token.ok_or("Cloud login required for Jev")?)?,
        cloud: true,
        team_id,
        project_id: crate::cloud::resolve_canonical_id(cas_root),
    })
}
fn valid_token(token: String) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() || token.chars().any(|c| c.is_control()) {
        return Err("Invalid Jev credential".into());
    }
    Ok(token.into())
}
fn retry_after(value: &str) -> Option<Duration> {
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|date| {
                    (date.with_timezone(&Utc) - Utc::now())
                        .to_std()
                        .unwrap_or_default()
                })
        })
}

pub fn validate_input(state: &Value, questions: &Value) -> Result<(), JevError> {
    let invalid = || {
        JevError::InvalidInput("Jev requires string/object/array state and a nonempty map of typed questions (noul, choice or score)".into())
    };
    if !(state.is_string() || state.is_object() || state.is_array()) {
        return Err(invalid());
    }
    let questions = questions
        .as_object()
        .filter(|q| !q.is_empty())
        .ok_or_else(invalid)?;
    for question in questions.values() {
        let instructions = question.get("instructions").ok_or_else(invalid)?;
        if !(instructions.is_string() || instructions.is_object() || instructions.is_array()) {
            return Err(invalid());
        }
        match question.get("type").and_then(Value::as_str) {
            Some("noul") => {
                if question.get("criteria").is_some_and(|c| !c.is_object()) {
                    return Err(invalid());
                }
            }
            Some("choice") => {
                if !question
                    .get("criteria")
                    .and_then(Value::as_object)
                    .is_some_and(|c| (1..=255).contains(&c.len()))
                {
                    return Err(invalid());
                }
            }
            Some("score") => {
                if !question
                    .get("criteria")
                    .and_then(Value::as_array)
                    .is_some_and(|c| (2..=10).contains(&c.len()))
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
    }
    Ok(())
}
fn validate_response(response: &Response, questions: &Value) -> Result<(), JevError> {
    let invalid = || JevError::Unavailable("Invalid Jev answer shape or probabilities".into());
    let questions = questions.as_object().expect("validated questions");
    if response.model.is_empty() || response.answers.len() != questions.len() {
        return Err(invalid());
    }
    let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
    for (id, question) in questions {
        let answer = response.answers.get(id).ok_or_else(invalid)?;
        match (question["type"].as_str(), answer) {
            (Some("noul"), Answer::Noul { noul }) if unit(*noul) => {}
            (
                Some("choice"),
                Answer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) if unit(*confidence) && probabilities.contains_key(choice) => {
                let criteria = question["criteria"]
                    .as_object()
                    .expect("validated choice criteria");
                if probabilities.len() != criteria.len()
                    || !criteria.keys().all(|k| probabilities.contains_key(k))
                {
                    return Err(invalid());
                }
                validate_probabilities(probabilities)?;
            }
            (
                Some("score"),
                Answer::Score {
                    score,
                    legend,
                    probabilities,
                    confidence,
                },
            ) if unit(*confidence) && score.is_finite() => {
                let count = question["criteria"]
                    .as_array()
                    .expect("validated score criteria")
                    .len();
                if !(0.0..=(count - 1) as f64).contains(score)
                    || legend.len() != count
                    || probabilities.len() != count
                    || !(0..count).all(|i| {
                        legend.contains_key(&i.to_string())
                            && probabilities.contains_key(&i.to_string())
                    })
                {
                    return Err(invalid());
                }
                validate_probabilities(probabilities)?;
            }
            _ => return Err(invalid()),
        }
    }
    Ok(())
}
fn validate_probabilities(values: &BTreeMap<String, f64>) -> Result<(), JevError> {
    if values
        .values()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        || (values.values().sum::<f64>() - 1.0).abs() > 0.02
    {
        return Err(JevError::Unavailable(
            "Invalid Jev probability distribution".into(),
        ));
    }
    Ok(())
}
fn append_log(path: &Path, row: &Value) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.lock_exclusive()?;
    let mut bytes = serde_json::to_vec(row)?;
    bytes.push(b'\n');
    let result = file.write_all(&bytes).and_then(|_| file.sync_data());
    let unlock = FileExt::unlock(&file);
    result.and(unlock)
}

mod files;
pub mod gate;
pub use files::{DEFAULT_FILE_BYTES, FileRow, FilesOptions, FilesResponse, MAX_FILE_BYTES};

#[cfg(test)]
mod tests;
