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

/// Codex writes one JSON object per line; the fields we need live under
/// `payload` on `event_msg` records.
///
/// A turn is an account failure when its `task_complete` carries an `error`
/// whose `codex_error_info` names an authorization problem. `last_agent_message`
/// being null is what distinguishes "died before saying anything" from "worked,
/// then hit a wall", and it is recorded in the message so the supervisor can
/// tell the two apart.
pub fn codex_rollout_auth_failure(tail: &str) -> AuthFailureEvidence {
    let mut latest: Option<AuthFailureEvidence> = None;
    for (index, line) in tail.lines().enumerate() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = record.get("payload").unwrap_or(&serde_json::Value::Null);
        if payload.get("type").and_then(serde_json::Value::as_str) != Some("task_complete") {
            continue;
        }
        let error = payload.get("error");
        let info = error
            .and_then(|error| error.get("codex_error_info"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !codex_error_info_is_auth(info) {
            // Any terminal turn that did not fail on the account closes the
            // episode, including one that failed for an unrelated reason: the
            // harness reached the model, so the credential worked.
            latest = Some(AuthFailureEvidence::Healthy);
            continue;
        }
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Codex refused the turn as unauthorized")
            .to_string();
        let occurrence = record
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| format!("line-{index}"), str::to_owned);
        latest = Some(AuthFailureEvidence::Failed {
            message,
            occurrence,
        });
    }
    latest.unwrap_or(AuthFailureEvidence::Unavailable)
}

fn codex_error_info_is_auth(info: &str) -> bool {
    matches!(
        info.to_ascii_lowercase().as_str(),
        "unauthorized" | "unauthenticated" | "auth_error" | "invalid_credentials"
    )
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
    format!(
        "Worker '{worker}' never started work: its {} harness ended the first turn with an account failure — {message} \
         The worker process may still be heartbeating, so this is not visible as a dead worker. {}",
        harness_label(cli),
        auth_failure_remedy(cli, account_dir),
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
