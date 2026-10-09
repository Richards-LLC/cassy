//! Harness account-health evidence read from a worker's own transcript.
//!
//! A worker whose harness refuses its very first turn for an account reason —
//! a revoked Codex refresh token, an expired Claude OAuth session — is not a
//! slow worker. It is a dead one that will heartbeat forever: the harness
//! process stays up, the MCP child stays registered, and the only trace is one
//! line in a rollout file. In the incident this module exists for, four Codex
//! workers each ended their first turn in ~1.2s with
//! `codex_error_info: "unauthorized"` and were still listed as live, assigned
//! and unstarted 34 minutes later.
//!
//! The scanners here are deliberately pure over transcript text so the
//! incident's own rollout can be replayed as a fixture, and deliberately
//! "latest terminal turn wins" so a transient failure the harness itself
//! retried past cannot kill a working worker.

/// What a transcript says about the account behind a harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFailureEvidence {
    /// The most recent terminal turn failed for an account reason.
    Failed {
        /// The harness's own message, already free of secrets.
        message: String,
        /// Durable identity for this episode (timestamp when available), so a
        /// relay is sent once per failure rather than once per scan.
        occurrence: String,
    },
    /// A terminal turn completed without an account error, or the transcript
    /// belongs to a harness this scanner does not read.
    Healthy,
    /// Nothing could be read. Explicitly not "healthy": an unreadable
    /// transcript must never be used to close an open episode.
    Unavailable,
}

impl AuthFailureEvidence {
    pub fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// What one Codex transcript record says about the account behind it.
///
/// Shared by the daemon's blocker relay ([`codex_rollout_auth_failure`]) and
/// the worker liveness reader, so both agree on which provider errors are
/// fatal (cas-5e3c, GH #1130).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRecordVerdict {
    /// The provider refused the turn for a reason no retry fixes: the account
    /// is unauthorized, or it cannot use the requested model.
    Fatal(String),
    /// A terminal turn that did not fail fatally: the harness reached the
    /// model, so the account and model work.
    Healthy,
    /// Any other record, including non-fatal errors such as a dropped stream.
    Neutral,
}

/// Classify one Codex record. Only structured error records count; a tool
/// output that merely quotes an error string is `Neutral`.
///
/// Three shapes carry a fatal refusal:
/// - a rollout `event_msg` `task_complete` whose `error` has an authorization
///   `codex_error_info`, or whose `error.message` names a refused model (the
///   real shape of the GH #1130 rollouts: `codex_error_info: "other"` with the
///   provider's 400 body as a JSON string);
/// - a rollout `event_msg` whose payload `type` is `error`;
/// - a bare `{"type":"error","status":400,"error":{...}}` record.
pub fn codex_record_verdict(record: &serde_json::Value) -> CodexRecordVerdict {
    let kind = record.get("type").and_then(serde_json::Value::as_str);
    if kind == Some("error") {
        return fatal_error_message(record).map_or(CodexRecordVerdict::Neutral, CodexRecordVerdict::Fatal);
    }
    if kind != Some("event_msg") {
        return CodexRecordVerdict::Neutral;
    }
    let payload = record.get("payload").unwrap_or(&serde_json::Value::Null);
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("error") => {
            fatal_error_message(payload).map_or(CodexRecordVerdict::Neutral, CodexRecordVerdict::Fatal)
        }
        Some("task_complete" | "turn_completed") => match payload.get("error") {
            Some(error) if !error.is_null() => fatal_error_message(error)
                // Any terminal turn that did not fail fatally closes the
                // episode, including one that failed for an unrelated reason:
                // the harness reached the model, so the credential worked.
                .map_or(CodexRecordVerdict::Healthy, CodexRecordVerdict::Fatal),
            _ => CodexRecordVerdict::Healthy,
        },
        _ => CodexRecordVerdict::Neutral,
    }
}

/// The provider's message when `error` is a fatal account or model refusal.
fn fatal_error_message(error: &serde_json::Value) -> Option<String> {
    let info = error
        .get("codex_error_info")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let message = innermost_error_message(error);
    if codex_error_info_is_auth(info) {
        return Some(message.unwrap_or_else(|| "Codex refused the turn as unauthorized".to_string()));
    }
    message.filter(|message| message_is_model_refusal(message))
}

/// Codex nests the provider body as a JSON string inside `message`; unwrap it
/// so the supervisor reads the provider's sentence rather than escaped JSON.
fn innermost_error_message(value: &serde_json::Value) -> Option<String> {
    fn walk(value: &serde_json::Value, depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        match value {
            serde_json::Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.starts_with('{')
                    && let Ok(inner) = serde_json::from_str::<serde_json::Value>(trimmed)
                    && let Some(found) = walk(&inner, depth + 1)
                {
                    return Some(found);
                }
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            serde_json::Value::Object(object) => ["error", "message"]
                .into_iter()
                .filter_map(|key| object.get(key))
                .find_map(|child| walk(child, depth + 1)),
            _ => None,
        }
    }
    walk(value, 0)
}

/// A provider refusal of the requested model, which no retry can fix: the
/// ChatGPT-account wording from GH #1130 and the generic unknown-model forms.
/// "Selected model is at capacity" deliberately does not match.
pub fn message_is_model_refusal(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("not supported when using codex with a chatgpt account") {
        return true;
    }
    lowered.contains("model")
        && (lowered.contains("is not supported")
            || lowered.contains("does not exist")
            || lowered.contains("model_not_found")
            || lowered.contains("do not have access"))
}

/// Codex writes one JSON object per line; the fields we need live under
/// `payload` on `event_msg` records. See [`codex_record_verdict`] for which
/// records count. `last_agent_message` being null is what distinguishes
/// "died before saying anything" from "worked, then hit a wall".
pub fn codex_rollout_auth_failure(tail: &str) -> AuthFailureEvidence {
    let mut latest: Option<AuthFailureEvidence> = None;
    for (index, line) in tail.lines().enumerate() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match codex_record_verdict(&record) {
            CodexRecordVerdict::Neutral => {}
            CodexRecordVerdict::Healthy => latest = Some(AuthFailureEvidence::Healthy),
            CodexRecordVerdict::Fatal(message) => {
                let occurrence = record
                    .get("timestamp")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| format!("line-{index}"), str::to_owned);
                latest = Some(AuthFailureEvidence::Failed {
                    message,
                    occurrence,
                });
            }
        }
    }
    latest.unwrap_or(AuthFailureEvidence::Unavailable)
}

fn codex_error_info_is_auth(info: &str) -> bool {
    matches!(
        info.to_ascii_lowercase().as_str(),
        "unauthorized" | "unauthenticated" | "auth_error" | "invalid_credentials"
    )
}

/// Whether a Codex account can run a model, read from its own `CODEX_HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexModelSupport {
    /// The account's catalogue lists the model, or the account is not a
    /// ChatGPT account (API-key accounts are not restricted by this list).
    Supported,
    /// A fresh ChatGPT-account catalogue that does not list the model: every
    /// turn would die with "not supported when using Codex with a ChatGPT
    /// account" (GH #1130).
    Unsupported { catalog: std::path::PathBuf, listed: Vec<String> },
    /// No usable evidence. Not a refusal: a missing or stale catalogue says
    /// nothing about a model released since it was written.
    Unverified(String),
}

/// A catalogue older than this may predate the model, so it cannot refuse it.
const CODEX_CATALOG_MAX_AGE_DAYS: i64 = 7;

/// Codex records the ChatGPT account's model list in `models_cache.json`
/// beside `auth.json`; a model absent from a fresh list is one the account
/// cannot run (cas-5e3c).
pub fn codex_account_model_support(
    codex_home: &std::path::Path,
    model: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> CodexModelSupport {
    let read_json = |name: &str| -> Result<serde_json::Value, String> {
        let path = codex_home.join(name);
        let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
    };
    let auth = match read_json("auth.json") {
        Ok(auth) => auth,
        Err(error) => return CodexModelSupport::Unverified(error),
    };
    let mode = auth.get("auth_mode").and_then(serde_json::Value::as_str).unwrap_or_default();
    if !mode.eq_ignore_ascii_case("chatgpt") {
        return CodexModelSupport::Supported;
    }
    let catalog = match read_json("models_cache.json") {
        Ok(catalog) => catalog,
        Err(error) => return CodexModelSupport::Unverified(error),
    };
    let fresh = catalog
        .get("fetched_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| (now - at.with_timezone(&chrono::Utc)).num_days() < CODEX_CATALOG_MAX_AGE_DAYS);
    if !fresh {
        return CodexModelSupport::Unverified("models_cache.json is missing fetched_at or is stale".to_string());
    }
    let listed: Vec<String> = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("slug").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect();
    if listed.is_empty() {
        return CodexModelSupport::Unverified("models_cache.json lists no models".to_string());
    }
    if listed.iter().any(|slug| slug.eq_ignore_ascii_case(model.trim())) {
        CodexModelSupport::Supported
    } else {
        CodexModelSupport::Unsupported {
            catalog: codex_home.join("models_cache.json"),
            listed,
        }
    }
}

/// Claude's JSONL transcript carries assistant/user records rather than a
/// terminal turn marker, so the account signal is an error record whose text
/// names an authentication failure, with any later assistant output closing
/// the episode.
pub fn claude_transcript_auth_failure(tail: &str) -> AuthFailureEvidence {
    let mut latest: Option<AuthFailureEvidence> = None;
    for (index, line) in tail.lines().enumerate() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = record
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if kind == "assistant" {
            // The model answered, so whatever the credential state was, it
            // worked. This is the guard against killing a worker over a
            // transient 401 the harness retried past.
            latest = Some(AuthFailureEvidence::Healthy);
            continue;
        }
        let text = claude_record_text(&record);
        if text.is_empty() || !claude_text_is_auth_failure(&text) {
            continue;
        }
        let occurrence = record
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| format!("line-{index}"), str::to_owned);
        latest = Some(AuthFailureEvidence::Failed {
            message: text,
            occurrence,
        });
    }
    latest.unwrap_or(AuthFailureEvidence::Unavailable)
}

fn claude_record_text(record: &serde_json::Value) -> String {
    for key in ["error", "message", "result", "subtype"] {
        match record.get(key) {
            Some(serde_json::Value::String(text)) => return text.clone(),
            Some(value @ serde_json::Value::Object(_)) => {
                if let Some(text) = value.get("message").and_then(serde_json::Value::as_str) {
                    return text.to_string();
                }
            }
            _ => {}
        }
    }
    String::new()
}

fn claude_text_is_auth_failure(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    // Both halves must match: "401" alone appears in ordinary tool output, and
    // an agent discussing authentication is not an authentication failure.
    let names_auth = lowered.contains("oauth")
        || lowered.contains("api key")
        || lowered.contains("authentication")
        || lowered.contains("credentials")
        || lowered.contains("401");
    let names_failure = lowered.contains("invalid")
        || lowered.contains("expired")
        || lowered.contains("revoked")
        || lowered.contains("unauthorized")
        || lowered.contains("please run /login")
        || lowered.contains("please log in")
        || lowered.contains("log out and sign in");
    names_auth && names_failure
}

/// What the operator has to do, naming the account the worker actually used.
/// A remedy that does not name the directory is unactionable on a host with
/// several accounts, which is exactly the host this runs on.
pub fn auth_failure_remedy(cli: cas_mux::SupervisorCli, account_dir: Option<&str>) -> String {
    match cli {
        cas_mux::SupervisorCli::Codex => match account_dir {
            Some(dir) if !dir.trim().is_empty() => format!(
                "Run `CODEX_HOME={} codex login` on this host, then re-issue the spawn.",
                dir.trim()
            ),
            _ => "Run `codex login` on this host (default CODEX_HOME ~/.codex), then re-issue the spawn.".to_string(),
        },
        cas_mux::SupervisorCli::Claude => match account_dir {
            Some(dir) if !dir.trim().is_empty() => format!(
                "Run `CLAUDE_CONFIG_DIR={} claude login` on this host, then re-issue the spawn.",
                dir.trim()
            ),
            _ => "Run `claude login` on this host, then re-issue the spawn.".to_string(),
        },
        other => format!(
            "Re-authenticate the {} account on this host, then re-issue the spawn.",
            harness_label(other)
        ),
    }
}

fn harness_label(cli: cas_mux::SupervisorCli) -> &'static str {
    match cli {
        cas_mux::SupervisorCli::Claude => "claude",
        cas_mux::SupervisorCli::Codex => "codex",
        cas_mux::SupervisorCli::Grok => "grok",
        cas_mux::SupervisorCli::OpenCode => "opencode",
    }
}

/// The supervisor-facing sentence for a worker killed by its account.
pub fn auth_failure_detail(
    worker: &str,
    cli: cas_mux::SupervisorCli,
    account_dir: Option<&str>,
    message: &str,
) -> String {
    // cas-5e3c: an account that cannot use the requested model is not fixed
    // by logging in again; name the remedy that actually works.
    let (cause, remedy) = if message_is_model_refusal(message) {
        ("a model its account cannot use", model_refusal_remedy(account_dir))
    } else {
        ("an account failure", auth_failure_remedy(cli, account_dir))
    };
    format!(
        "Worker '{worker}' never started work: its {} harness ended the first turn with {cause} — {message} \
         The worker process may still be heartbeating, so this is not visible as a dead worker. {remedy}",
        harness_label(cli),
    )
}

/// What to do when the account behind a worker refuses its model.
pub fn model_refusal_remedy(account_dir: Option<&str>) -> String {
    let account = account_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| "the default CODEX_HOME (~/.codex)".to_string(), |dir| format!("CODEX_HOME={dir}"));
    format!(
        "Re-issue the spawn with a model listed in {account}'s models_cache.json (explicit `model=`, or another lane), \
         or with config_dir naming an account that supports it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape of the four rollouts from the 2026-09-03 incident.
    const UNAUTHORIZED_FIRST_TURN: &str = r#"{"timestamp":"2026-09-03T14:12:58.000Z","type":"session_meta","payload":{"id":"01a06879"}}
{"timestamp":"2026-09-03T14:12:59.000Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1","started_at":1788459179}}
{"timestamp":"2026-09-03T14:13:01.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":null,"error":{"message":"Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.","codex_error_info":"unauthorized"},"duration_ms":2428}}"#;

    const HEALTHY_FIRST_TURN: &str = r#"{"timestamp":"2026-09-03T14:12:58.000Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}
{"timestamp":"2026-09-03T14:13:30.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"Started cas-1234."}}"#;

    #[test]
    fn unsupported_chatgpt_model_is_a_relayable_failure_until_success() {
        let failure = r#"{"type":"event_msg","payload":{"type":"error","message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account.\"}}"}}"#;
        let evidence = codex_rollout_auth_failure(failure);
        assert!(evidence.failed(), "{evidence:?}");
        assert_eq!(codex_rollout_auth_failure(&format!("{failure}\n{HEALTHY_FIRST_TURN}")), AuthFailureEvidence::Healthy);
    }

    /// Verbatim shape of a real rollout whose ChatGPT account refused the
    /// model: `codex_error_info` is "other" and the 400 body is a JSON string.
    const MODEL_REFUSED_TASK_COMPLETE: &str = r#"{"timestamp":"2026-08-18T16:12:37.830Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":null,"error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-5.6-sol' model is not supported when using Codex with a ChatGPT account.\"}}","codex_error_info":"other"},"duration_ms":1568}}"#;

    #[test]
    fn a_real_model_refused_task_complete_is_a_failure_with_the_providers_sentence() {
        let AuthFailureEvidence::Failed { message, occurrence } =
            codex_rollout_auth_failure(MODEL_REFUSED_TASK_COMPLETE)
        else {
            panic!("expected a fatal model refusal");
        };
        assert_eq!(
            message,
            "The 'gpt-5.6-sol' model is not supported when using Codex with a ChatGPT account."
        );
        assert_eq!(occurrence, "2026-08-18T16:12:37.830Z");
        let detail = auth_failure_detail("w", cas_mux::SupervisorCli::Codex, Some("~/.codex-alt"), &message);
        assert!(detail.contains("a model its account cannot use"), "{detail}");
        assert!(detail.contains("CODEX_HOME=~/.codex-alt"), "{detail}");
        assert!(!detail.contains("codex login"), "{detail}");
    }

    #[test]
    fn a_chatgpt_catalogue_decides_model_support_only_when_fresh() {
        let home = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let write = |mode: &str, fetched: chrono::DateTime<chrono::Utc>| {
            std::fs::write(home.path().join("auth.json"), format!(r#"{{"auth_mode":"{mode}"}}"#)).unwrap();
            std::fs::write(
                home.path().join("models_cache.json"),
                serde_json::json!({"fetched_at": fetched, "models": [{"slug": "gpt-6-sol"}]}).to_string(),
            )
            .unwrap();
        };
        assert!(matches!(
            codex_account_model_support(home.path(), "gpt-6.1-sol", now),
            CodexModelSupport::Unverified(_)
        ));
        write("chatgpt", now);
        assert_eq!(codex_account_model_support(home.path(), "gpt-6-sol", now), CodexModelSupport::Supported);
        assert!(matches!(
            codex_account_model_support(home.path(), "gpt-6.1-sol", now),
            CodexModelSupport::Unsupported { ref listed, .. } if listed == &["gpt-6-sol"]
        ));
        write("chatgpt", now - chrono::Duration::days(30));
        assert!(matches!(
            codex_account_model_support(home.path(), "gpt-6.1-sol", now),
            CodexModelSupport::Unverified(_)
        ));
        write("apikey", now);
        assert_eq!(codex_account_model_support(home.path(), "gpt-6.1-sol", now), CodexModelSupport::Supported);
    }

    #[test]
    fn capacity_errors_and_quoted_error_text_are_not_fatal() {
        let capacity = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","error":{"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"}}}"#;
        assert_eq!(codex_rollout_auth_failure(capacity), AuthFailureEvidence::Healthy);
        let quoted = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","output":"The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account."}}"#;
        assert_eq!(codex_rollout_auth_failure(quoted), AuthFailureEvidence::Unavailable);
    }

    #[test]
    fn codex_unauthorized_first_turn_is_an_account_failure_with_its_message() {
        let evidence = codex_rollout_auth_failure(UNAUTHORIZED_FIRST_TURN);
        let AuthFailureEvidence::Failed {
            message,
            occurrence,
        } = evidence
        else {
            panic!("expected an account failure, got {evidence:?}");
        };
        assert!(message.contains("refresh token was revoked"), "{message}");
        assert_eq!(occurrence, "2026-09-03T14:13:01.000Z");
    }

    #[test]
    fn codex_healthy_turn_is_not_an_account_failure() {
        assert_eq!(
            codex_rollout_auth_failure(HEALTHY_FIRST_TURN),
            AuthFailureEvidence::Healthy
        );
    }

    #[test]
    fn a_transient_unauthorized_turn_followed_by_a_completed_turn_does_not_kill_a_worker() {
        // The whole reason evidence is "latest terminal turn wins": Codex
        // retries past transient authorization failures, and a worker that is
        // demonstrably working must not be killed by an old line.
        let tail = format!("{UNAUTHORIZED_FIRST_TURN}\n{HEALTHY_FIRST_TURN}");
        assert_eq!(
            codex_rollout_auth_failure(&tail),
            AuthFailureEvidence::Healthy
        );
    }

    #[test]
    fn a_turn_that_failed_for_an_unrelated_reason_is_not_an_account_failure() {
        let tail = r#"{"timestamp":"2026-09-03T14:13:01.000Z","type":"event_msg","payload":{"type":"task_complete","error":{"message":"stream disconnected before completion","codex_error_info":"stream_error"}}}"#;
        assert_eq!(codex_rollout_auth_failure(tail), AuthFailureEvidence::Healthy);
    }

    #[test]
    fn an_empty_or_unreadable_rollout_is_unavailable_rather_than_healthy() {
        assert_eq!(codex_rollout_auth_failure(""), AuthFailureEvidence::Unavailable);
        assert_eq!(
            codex_rollout_auth_failure("not json\nalso not json"),
            AuthFailureEvidence::Unavailable
        );
    }

    #[test]
    fn claude_oauth_expiry_is_an_account_failure_and_an_answer_closes_it() {
        let failure = r#"{"timestamp":"2026-09-03T14:13:01.000Z","type":"error","error":{"message":"OAuth token expired. Please run /login to authenticate."}}"#;
        assert!(claude_transcript_auth_failure(failure).failed());

        let recovered = format!(
            "{failure}\n{}",
            r#"{"timestamp":"2026-09-03T14:14:00.000Z","type":"assistant","message":{"content":"working on it"}}"#
        );
        assert_eq!(
            claude_transcript_auth_failure(&recovered),
            AuthFailureEvidence::Healthy
        );
    }

    #[test]
    fn claude_prose_about_authentication_is_not_an_account_failure() {
        // An agent reading a 401 out of a curl it ran, or discussing an API
        // key, must not be mistaken for a harness that cannot authenticate.
        let tail = r#"{"timestamp":"2026-09-03T14:13:01.000Z","type":"user","message":{"content":"the docs mention an api key and authentication"}}
{"timestamp":"2026-09-03T14:13:02.000Z","type":"error","error":{"message":"curl returned 401 from the vendor sandbox"}}"#;
        assert_eq!(
            claude_transcript_auth_failure(tail),
            AuthFailureEvidence::Unavailable
        );
    }

    #[test]
    fn the_remedy_names_the_account_directory_the_worker_used() {
        let remedy = auth_failure_remedy(cas_mux::SupervisorCli::Codex, Some("~/.codex-alt"));
        assert!(remedy.contains("CODEX_HOME=~/.codex-alt"), "{remedy}");
        assert!(remedy.contains("codex login"), "{remedy}");

        let default = auth_failure_remedy(cas_mux::SupervisorCli::Codex, None);
        assert!(default.contains("~/.codex"), "{default}");

        let claude = auth_failure_remedy(cas_mux::SupervisorCli::Claude, Some("~/.claude-alt"));
        assert!(claude.contains("CLAUDE_CONFIG_DIR=~/.claude-alt"), "{claude}");
    }

    #[test]
    fn the_supervisor_detail_names_the_worker_the_cause_and_the_remedy() {
        let detail = auth_failure_detail(
            "zen-eagle-20",
            cas_mux::SupervisorCli::Codex,
            None,
            "Your access token could not be refreshed because your refresh token was revoked.",
        );
        assert!(detail.contains("zen-eagle-20"), "{detail}");
        assert!(detail.contains("refresh token was revoked"), "{detail}");
        assert!(detail.contains("codex login"), "{detail}");
        assert!(detail.contains("never started work"), "{detail}");
    }
}
