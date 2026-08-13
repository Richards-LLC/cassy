//! Artifact-backed harness turn observations for supervisor-facing status.
//!
//! The evidence matrix is intentionally asymmetric:
//!
//! - **Claude:** Agent Teams inbox persistence proves transport delivery, not
//!   worker wake. The session transcript does: a top-level `user` record with
//!   textual content is a concrete turn start, while `system/turn_duration`
//!   (or an assistant `end_turn`) is a concrete turn end. Tool-result `user`
//!   records are deliberately excluded from the start watermark.
//! - **Codex:** a per-message observation requires an exact queued-prompt match
//!   in a user `response_item`, whose harness `turn_id` ties it to a concrete
//!   turn. `task_started` records the wake of a new turn; when a message is
//!   steered into an already-active turn, the matched user record itself is
//!   concrete consumption evidence. A later assistant `response_item` with the
//!   same `turn_id` is reaction evidence.
//! - **Grok:** `updates.jsonl` exposes turn starts for worker-level status, and
//!   sibling `events.jsonl` exposes `turn_ended` completion. Neither artifact
//!   currently exposes a stable message/turn correlation CAS can support, so
//!   per-message wake and reaction remain unobserved rather than being inferred
//!   from an unrelated later turn.
//!
//! Codex urgent and non-urgent injections travel through different delivery
//! paths (interrupt-then-inject versus direct injection) and have different
//! failure modes. Observations are therefore correlated per queued prompt and
//! harness turn, never aggregated from the harness type or delivery path.
//!
//! No observation is inferred from queue age, heartbeat age, file mtime, or a
//! successful inbox/PTY write. Missing, unreadable, or unfamiliar artifacts
//! fail closed to `None`.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactObservation {
    pub at: DateTime<Utc>,
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HarnessObservations {
    pub wake: Option<ArtifactObservation>,
    pub reaction: Option<ArtifactObservation>,
    pub completion: Option<ArtifactObservation>,
}

/// Observe the first concrete wake/reaction records at or after delivery.
pub(crate) fn observations_after_delivery(
    artifact_path: &Path,
    cli: cas_mux::SupervisorCli,
    delivered_at: DateTime<Utc>,
    prompt: &str,
) -> HarnessObservations {
    match cli {
        cas_mux::SupervisorCli::Claude => scan_claude_message(artifact_path, delivered_at, prompt),
        cas_mux::SupervisorCli::Codex => scan_codex_message(artifact_path, delivered_at, prompt),
        cas_mux::SupervisorCli::Grok => scan_grok_completion(artifact_path, delivered_at),
    }
}

/// Observe the latest concrete turn records for `worker_status`.
pub(crate) fn latest_turn_observations(
    artifact_path: &Path,
    cli: cas_mux::SupervisorCli,
) -> HarnessObservations {
    match cli {
        cas_mux::SupervisorCli::Claude => scan_claude_turns(artifact_path, None, true),
        cas_mux::SupervisorCli::Codex => scan_codex_turns(artifact_path, None, true),
        cas_mux::SupervisorCli::Grok => scan_grok(artifact_path, None, true),
    }
}

fn scan_claude_message(
    path: &Path,
    delivered_at: DateTime<Utc>,
    prompt: &str,
) -> HarnessObservations {
    if prompt.is_empty() {
        return HarnessObservations::default();
    }

    let mut matched_wake = None;
    let mut observations = HarnessObservations::default();
    for_each_jsonl(path, |value| {
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if at < delivered_at {
            return;
        }

        if matched_wake.is_none()
            && claude_user_text(value).is_some_and(|text| text_matches_queued_prompt(text, prompt))
        {
            matched_wake = Some(at);
            observations.wake = Some(ArtifactObservation {
                at,
                evidence: format!(
                    "Claude transcript {} contains a top-level textual user record matching the queued prompt",
                    path.display()
                ),
            });
            return;
        }

        let Some(wake_at) = matched_wake else {
            return;
        };
        if at < wake_at {
            return;
        }
        if observations.reaction.is_none() && claude_is_assistant(value) {
            observations.reaction = Some(ArtifactObservation {
                at,
                evidence: format!(
                    "Claude transcript {} contains an assistant record after the matched queued prompt",
                    path.display()
                ),
            });
        }
        if observations.completion.is_none() && claude_is_turn_end(value) {
            observations.completion = Some(ArtifactObservation {
                at,
                evidence: format!(
                    "Claude transcript {} contains an end_turn/turn_duration record after the matched queued prompt",
                    path.display()
                ),
            });
        }
    });
    observations
}

fn scan_claude_turns(
    path: &Path,
    after: Option<DateTime<Utc>>,
    latest: bool,
) -> HarnessObservations {
    let mut observations = HarnessObservations::default();
    for_each_jsonl(path, |value| {
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if after.is_some_and(|floor| at < floor) {
            return;
        }

        if claude_user_text(value).is_some() {
            record(
                &mut observations.wake,
                ArtifactObservation {
                    at,
                    evidence: format!(
                        "Claude transcript {} contains a top-level textual user turn-start record",
                        path.display()
                    ),
                },
                latest,
            );
            return;
        }

        let wake_at = observations.wake.as_ref().map(|observation| observation.at);
        if wake_at.is_some_and(|wake_at| at >= wake_at) && claude_is_assistant(value) {
            record(
                &mut observations.reaction,
                ArtifactObservation {
                    at,
                    evidence: format!(
                        "Claude transcript {} contains an assistant record after the observed turn start",
                        path.display()
                    ),
                },
                latest,
            );
        }
        if wake_at.is_some_and(|wake_at| at >= wake_at) && claude_is_turn_end(value) {
            record(
                &mut observations.completion,
                ArtifactObservation {
                    at,
                    evidence: format!(
                        "Claude transcript {} contains an end_turn/turn_duration record",
                        path.display()
                    ),
                },
                latest,
            );
        }
    });
    discard_reaction_before_wake(&mut observations);
    discard_completion_before_wake(&mut observations);
    observations
}

fn scan_codex_message(
    path: &Path,
    delivered_at: DateTime<Utc>,
    prompt: &str,
) -> HarnessObservations {
    if prompt.is_empty() {
        return HarnessObservations::default();
    }

    let mut starts = HashMap::<String, DateTime<Utc>>::new();
    let mut matched_inputs = Vec::<(DateTime<Utc>, String)>::new();
    let mut reactions = Vec::<(DateTime<Utc>, String)>::new();
    for_each_jsonl(path, |value| {
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if let Some(turn_id) = codex_turn_start_id(value) {
            starts.insert(turn_id.to_string(), at);
        }
        if at >= delivered_at {
            if let Some(turn_id) = codex_matching_user_turn(value, prompt) {
                matched_inputs.push((at, turn_id.to_string()));
            }
            if let Some(turn_id) = codex_assistant_turn(value) {
                reactions.push((at, turn_id.to_string()));
            }
        }
    });

    let Some((input_at, turn_id)) = matched_inputs.into_iter().min_by_key(|(at, _)| *at) else {
        return HarnessObservations::default();
    };
    let (wake_at, wake_shape) = match starts.get(&turn_id).copied() {
        Some(start_at) if start_at >= delivered_at && start_at <= input_at => (
            start_at,
            "matching user response_item plus task_started/turn_started",
        ),
        _ => (
            input_at,
            "matching user response_item consumed in an active turn",
        ),
    };

    let reaction = reactions
        .into_iter()
        .filter(|(at, candidate_turn)| *at >= input_at && candidate_turn == &turn_id)
        .min_by_key(|(at, _)| *at)
        .map(|(at, _)| ArtifactObservation {
            at,
            evidence: format!(
                "Codex rollout {} contains an assistant response_item after the matched queued prompt in turn_id={turn_id}",
                path.display()
            ),
        });

    HarnessObservations {
        wake: Some(ArtifactObservation {
            at: wake_at,
            evidence: format!(
                "Codex rollout {} contains {wake_shape} for the exact queued prompt in turn_id={turn_id}",
                path.display()
            ),
        }),
        reaction,
        completion: None,
    }
}

fn scan_codex_turns(
    path: &Path,
    after: Option<DateTime<Utc>>,
    latest: bool,
) -> HarnessObservations {
    let mut observations = HarnessObservations::default();
    for_each_jsonl(path, |value| {
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if after.is_some_and(|floor| at < floor) {
            return;
        }

        if codex_turn_start_id(value).is_some() {
            record(
                &mut observations.wake,
                ArtifactObservation {
                    at,
                    evidence: format!(
                        "Codex rollout {} contains event_msg.payload.type=task_started/turn_started",
                        path.display()
                    ),
                },
                latest,
            );
            return;
        }

        let wake_at = observations.wake.as_ref().map(|observation| observation.at);
        if wake_at.is_some_and(|wake_at| at >= wake_at) && codex_is_assistant_reaction(value) {
            record(
                &mut observations.reaction,
                ArtifactObservation {
                    at,
                    evidence: format!(
                        "Codex rollout {} contains an assistant response after the observed turn start",
                        path.display()
                    ),
                },
                latest,
            );
        }
    });
    discard_reaction_before_wake(&mut observations);
    observations
}

fn scan_grok_completion(path: &Path, delivered_at: DateTime<Utc>) -> HarnessObservations {
    let events_path = grok_events_path(path);
    let mut observations = HarnessObservations::default();
    for_each_jsonl(&events_path, |value| {
        if value.get("type").and_then(Value::as_str) != Some("turn_ended") {
            return;
        }
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if at < delivered_at {
            return;
        }
        record(
            &mut observations.completion,
            ArtifactObservation {
                at,
                evidence: format!(
                    "Grok events artifact {} contains turn_ended; this is completion only, not message-correlated wake evidence",
                    events_path.display()
                ),
            },
            false,
        );
    });
    observations
}

fn scan_grok(path: &Path, after: Option<DateTime<Utc>>, latest: bool) -> HarnessObservations {
    let mut observations = HarnessObservations::default();
    for_each_jsonl(path, |value| {
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if after.is_some_and(|floor| at < floor) {
            return;
        }
        let kind = value.get("type").and_then(Value::as_str);
        if matches!(kind, Some("turn_started" | "task_started")) {
            record(
                &mut observations.wake,
                ArtifactObservation {
                    at,
                    evidence: format!("Grok updates artifact {} contains {kind:?}", path.display()),
                },
                latest,
            );
            return;
        }
        let wake_at = observations.wake.as_ref().map(|observation| observation.at);
        if wake_at.is_some_and(|wake_at| at >= wake_at)
            && matches!(kind, Some("assistant_message" | "agent_message"))
        {
            record(
                &mut observations.reaction,
                ArtifactObservation {
                    at,
                    evidence: format!(
                        "Grok updates artifact {} contains an assistant/agent message after the observed turn start",
                        path.display()
                    ),
                },
                latest,
            );
        }
    });

    let events_path = grok_events_path(path);
    for_each_jsonl(&events_path, |value| {
        if value.get("type").and_then(Value::as_str) != Some("turn_ended") {
            return;
        }
        let Some(at) = json_timestamp(value) else {
            return;
        };
        if after.is_some_and(|floor| at < floor) {
            return;
        }
        record(
            &mut observations.completion,
            ArtifactObservation {
                at,
                evidence: format!(
                    "Grok events artifact {} contains turn_ended",
                    events_path.display()
                ),
            },
            latest,
        );
    });
    discard_reaction_before_wake(&mut observations);
    observations
}

fn discard_reaction_before_wake(observations: &mut HarnessObservations) {
    if observations
        .wake
        .as_ref()
        .zip(observations.reaction.as_ref())
        .is_some_and(|(wake, reaction)| reaction.at < wake.at)
    {
        observations.reaction = None;
    }
}

fn discard_completion_before_wake(observations: &mut HarnessObservations) {
    if observations
        .wake
        .as_ref()
        .zip(observations.completion.as_ref())
        .is_some_and(|(wake, completion)| completion.at < wake.at)
    {
        observations.completion = None;
    }
}

fn record(slot: &mut Option<ArtifactObservation>, candidate: ArtifactObservation, latest: bool) {
    let replace = match slot {
        None => true,
        Some(existing) if latest => candidate.at >= existing.at,
        Some(existing) => candidate.at < existing.at,
    };
    if replace {
        *slot = Some(candidate);
    }
}

fn for_each_jsonl(path: &Path, mut visit: impl FnMut(&Value)) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        visit(&value);
    }
}

fn json_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn claude_user_text(value: &Value) -> Option<&str> {
    if value.get("type").and_then(Value::as_str) != Some("user")
        || value.pointer("/message/role").and_then(Value::as_str) != Some("user")
        || value.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = value.pointer("/message/content")?;
    if let Some(text) = content.as_str() {
        return (!text.trim().is_empty()).then_some(text);
    }
    content.as_array()?.iter().find_map(|part| {
        (part.get("type").and_then(Value::as_str) == Some("text"))
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
            .filter(|text| !text.trim().is_empty())
    })
}

fn claude_is_assistant(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("assistant")
        && value.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
        && value.get("isSidechain").and_then(Value::as_bool) != Some(true)
}

fn claude_is_turn_end(value: &Value) -> bool {
    (value.get("type").and_then(Value::as_str) == Some("system")
        && value.get("subtype").and_then(Value::as_str) == Some("turn_duration"))
        || (claude_is_assistant(value)
            && value
                .pointer("/message/stop_reason")
                .and_then(Value::as_str)
                == Some("end_turn"))
}

fn codex_turn_start_id(value: &Value) -> Option<&str> {
    let payload = value.get("payload")?;
    (value.get("type").and_then(Value::as_str) == Some("event_msg")
        && payload
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "task_started" | "turn_started")))
    .then(|| payload.get("turn_id").and_then(Value::as_str))
    .flatten()
}

fn codex_matching_user_turn<'a>(value: &'a Value, prompt: &str) -> Option<&'a str> {
    let payload = value.get("payload")?;
    if value.get("type").and_then(Value::as_str) != Some("response_item")
        || payload.get("type").and_then(Value::as_str) != Some("message")
        || payload.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    let matches = payload
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("input_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .any(|text| text_matches_queued_prompt(text, prompt));
    matches.then(|| codex_response_turn_id(payload)).flatten()
}

fn text_matches_queued_prompt(text: &str, prompt: &str) -> bool {
    if text == prompt {
        return true;
    }
    if text.starts_with("<teammate-message ") && text.contains(prompt) {
        return true;
    }
    text.strip_suffix(prompt)
        .is_some_and(|prefix| prefix.starts_with("Message from ") && prefix.ends_with(": "))
}

fn codex_assistant_turn(value: &Value) -> Option<&str> {
    let payload = value.get("payload")?;
    (value.get("type").and_then(Value::as_str) == Some("response_item")
        && payload.get("type").and_then(Value::as_str) == Some("message")
        && payload.get("role").and_then(Value::as_str) == Some("assistant"))
    .then(|| codex_response_turn_id(payload))
    .flatten()
}

fn codex_response_turn_id(payload: &Value) -> Option<&str> {
    payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|metadata| metadata.get("turn_id"))
        .and_then(Value::as_str)
}

fn codex_is_assistant_reaction(value: &Value) -> bool {
    let outer = value.get("type").and_then(Value::as_str);
    let payload = value.get("payload");
    (outer == Some("response_item")
        && payload
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            == Some("message")
        && payload
            .and_then(|payload| payload.get("role"))
            .and_then(Value::as_str)
            == Some("assistant"))
        || (outer == Some("event_msg")
            && payload
                .and_then(|payload| payload.get("type"))
                .and_then(Value::as_str)
                == Some("agent_message"))
}

fn grok_events_path(updates_path: &Path) -> PathBuf {
    updates_path.with_file_name("events.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut file = std::fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn ts(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn codex_observes_artifact_records_after_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        write_lines(
            &rollout,
            &[
                r#"{"timestamp":"2026-07-31T20:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"old"}}"#,
                r#"{"timestamp":"2026-07-31T20:00:10Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"wanted"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Message from supervisor: act"}],"internal_chat_message_metadata_passthrough":{"turn_id":"wanted"}}}"#,
                r#"{"timestamp":"2026-07-31T20:01:03Z","type":"response_item","payload":{"type":"message","role":"assistant","internal_chat_message_metadata_passthrough":{"turn_id":"wanted"}}}"#,
            ],
        );

        let got = observations_after_delivery(
            &rollout,
            cas_mux::SupervisorCli::Codex,
            ts("2026-07-31T20:01:00Z"),
            "act",
        );
        assert_eq!(got.wake.unwrap().at, ts("2026-07-31T20:01:02Z"));
        assert_eq!(got.reaction.unwrap().at, ts("2026-07-31T20:01:03Z"));
    }

    #[test]
    fn elapsed_time_never_turns_an_unobserved_message_into_observed() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        write_lines(
            &rollout,
            &[
                r#"{"timestamp":"2026-07-31T20:00:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
                r#"{"timestamp":"2026-07-31T20:02:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"unrelated"}}"#,
                r#"{"timestamp":"2026-07-31T20:02:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Message from supervisor: a different prompt"}],"internal_chat_message_metadata_passthrough":{"turn_id":"unrelated"}}}"#,
                r#"{"timestamp":"2026-07-31T20:02:02Z","type":"response_item","payload":{"type":"message","role":"assistant","internal_chat_message_metadata_passthrough":{"turn_id":"unrelated"}}}"#,
            ],
        );

        let got = observations_after_delivery(
            &rollout,
            cas_mux::SupervisorCli::Codex,
            ts("2020-01-01T00:00:00Z"),
            "the prompt that never woke",
        );
        assert!(got.wake.is_none());
        assert!(got.reaction.is_none());
    }

    #[test]
    fn claude_transport_only_record_is_not_promoted_to_wake_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("claude.jsonl");
        write_lines(
            &transcript,
            &[r#"{"timestamp":"2026-07-31T20:01:02Z","type":"user","message":"inbox persisted"}"#],
        );
        let got = observations_after_delivery(
            &transcript,
            cas_mux::SupervisorCli::Claude,
            ts("2026-07-31T20:01:00Z"),
            "inbox persisted",
        );
        assert_eq!(got, HarnessObservations::default());
    }

    #[test]
    fn claude_teammate_transcript_records_turn_boundaries_and_reaction() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("claude.jsonl");
        write_lines(
            &transcript,
            &[
                r#"{"timestamp":"2026-07-31T20:00:00Z","type":"user","isSidechain":false,"message":{"role":"user","content":"old prompt"}}"#,
                r#"{"timestamp":"2026-07-31T20:00:01Z","type":"assistant","isSidechain":false,"message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"old answer"}]}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02Z","type":"user","isSidechain":false,"message":{"role":"user","content":"<teammate-message teammate_id=\"supervisor\">interrupt now</teammate-message>"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:03Z","type":"assistant","isSidechain":false,"message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"text","text":"ack"}]}}"#,
                r#"{"timestamp":"2026-07-31T20:01:04Z","type":"system","subtype":"turn_duration"}"#,
            ],
        );

        let got = observations_after_delivery(
            &transcript,
            cas_mux::SupervisorCli::Claude,
            ts("2026-07-31T20:01:00Z"),
            "interrupt now",
        );
        assert_eq!(got.wake.unwrap().at, ts("2026-07-31T20:01:02Z"));
        assert_eq!(got.reaction.unwrap().at, ts("2026-07-31T20:01:03Z"));
        assert_eq!(got.completion.unwrap().at, ts("2026-07-31T20:01:04Z"));
    }

    #[test]
    fn latest_claude_turn_ignores_tool_results_and_uses_textual_user_watermark() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("claude.jsonl");
        write_lines(
            &transcript,
            &[
                r#"{"timestamp":"2026-07-31T20:01:00Z","type":"user","isSidechain":false,"message":{"role":"user","content":"start work"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:01Z","type":"assistant","isSidechain":false,"message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_1"}]}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02Z","type":"user","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1"}]}}"#,
                r#"{"timestamp":"2026-07-31T20:01:03Z","type":"assistant","isSidechain":false,"message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}"#,
                r#"{"timestamp":"2026-07-31T20:01:04Z","type":"system","subtype":"turn_duration"}"#,
            ],
        );

        let got = latest_turn_observations(&transcript, cas_mux::SupervisorCli::Claude);
        assert_eq!(got.wake.unwrap().at, ts("2026-07-31T20:01:00Z"));
        assert_eq!(got.reaction.unwrap().at, ts("2026-07-31T20:01:03Z"));
        assert_eq!(got.completion.unwrap().at, ts("2026-07-31T20:01:04Z"));
    }

    #[test]
    fn grok_turn_end_is_completion_not_invented_wake() {
        let temp = tempfile::tempdir().unwrap();
        let updates = temp.path().join("updates.jsonl");
        let events = temp.path().join("events.jsonl");
        write_lines(&updates, &[]);
        write_lines(
            &events,
            &[r#"{"ts":"2026-07-31T20:01:05Z","type":"turn_ended","outcome":"completed"}"#],
        );
        let got = observations_after_delivery(
            &updates,
            cas_mux::SupervisorCli::Grok,
            ts("2026-07-31T20:01:00Z"),
            "act",
        );
        assert!(got.wake.is_none());
        assert_eq!(got.completion.unwrap().at, ts("2026-07-31T20:01:05Z"));
    }

    #[test]
    fn codex_steering_into_active_turn_is_concrete_consumption_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        write_lines(
            &rollout,
            &[
                r#"{"timestamp":"2026-07-31T20:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"active"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Message from supervisor: steer"}],"internal_chat_message_metadata_passthrough":{"turn_id":"active"}}}"#,
                r#"{"timestamp":"2026-07-31T20:01:03Z","type":"response_item","payload":{"type":"message","role":"assistant","internal_chat_message_metadata_passthrough":{"turn_id":"active"}}}"#,
            ],
        );

        let got = observations_after_delivery(
            &rollout,
            cas_mux::SupervisorCli::Codex,
            ts("2026-07-31T20:01:00Z"),
            "steer",
        );
        assert_eq!(got.wake.unwrap().at, ts("2026-07-31T20:01:02Z"));
        assert_eq!(got.reaction.unwrap().at, ts("2026-07-31T20:01:03Z"));
    }

    #[test]
    fn latest_codex_turn_selects_latest_start() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        write_lines(
            &rollout,
            &[
                r#"{"timestamp":"2026-07-31T20:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"first"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"second"}}"#,
            ],
        );
        let got = latest_turn_observations(&rollout, cas_mux::SupervisorCli::Codex);
        assert_eq!(got.wake.unwrap().at, ts("2026-07-31T20:01:02Z"));
    }

    #[test]
    fn latest_turn_does_not_reuse_an_older_turns_reaction() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        write_lines(
            &rollout,
            &[
                r#"{"timestamp":"2026-07-31T20:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"first"}}"#,
                r#"{"timestamp":"2026-07-31T20:00:01Z","type":"response_item","payload":{"type":"message","role":"assistant"}}"#,
                r#"{"timestamp":"2026-07-31T20:01:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"second"}}"#,
            ],
        );
        let got = latest_turn_observations(&rollout, cas_mux::SupervisorCli::Codex);
        assert_eq!(got.wake.unwrap().at, ts("2026-07-31T20:01:02Z"));
        assert!(got.reaction.is_none());
    }
}
