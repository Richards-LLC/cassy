use super::{MessageStageEvidence, StageStatusEvidence};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;

pub(crate) struct ClaudeAdapter;
pub(crate) struct CodexAdapter;
pub(crate) struct GrokAdapter;

#[derive(Clone, Debug)]
pub(crate) struct ClaudeFixture {
    pub inbox_path: PathBuf,
    pub transcript_path: Option<PathBuf>,
    pub message_id: String,
    pub target: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexFixture {
    pub rollout_path: PathBuf,
    pub message_id: String,
    pub target: String,
}

#[derive(Clone, Debug)]
pub(crate) struct GrokFixture {
    pub updates_path: PathBuf,
    pub events_path: Option<PathBuf>,
    pub message_id: String,
    pub target: String,
}

impl ClaudeAdapter {
    pub(crate) fn extract_fixture(fixture: &ClaudeFixture) -> Result<MessageStageEvidence> {
        let inbox: Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture.inbox_path)
                .with_context(|| format!("read Claude inbox {}", fixture.inbox_path.display()))?,
        )
        .context("parse Claude inbox JSON")?;
        let delivered_at_ms = inbox
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .filter(|message| value_contains_message_id(message, &fixture.message_id))
                    .filter_map(|message| {
                        message
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .and_then(timestamp_ms)
                    })
                    .min()
            })
            .context("Claude inbox did not contain correlated probe message")?;

        let first_reaction_at_ms = fixture
            .transcript_path
            .as_ref()
            .filter(|path| path.exists())
            .map(|path| {
                first_correlated_jsonl_timestamp(path, &fixture.message_id)
                    .with_context(|| format!("transcript_parse_failed: {}", path.display()))
            })
            .transpose()?
            .flatten()
            .filter(|ts| *ts >= delivered_at_ms);

        Ok(stage_from_parts(
            &fixture.message_id,
            &fixture.target,
            delivered_at_ms,
            None,
            first_reaction_at_ms,
        ))
    }
}

impl CodexAdapter {
    pub(crate) fn extract_fixture(fixture: &CodexFixture) -> Result<MessageStageEvidence> {
        let lines = read_jsonl(&fixture.rollout_path)
            .with_context(|| format!("read Codex rollout {}", fixture.rollout_path.display()))?;
        let delivered_at_ms = first_line_timestamp_matching(&lines, |value| {
            value_contains_message_id(value, &fixture.message_id)
                && value.get("type").and_then(Value::as_str) == Some("event_msg")
        })
        .context("Codex rollout did not contain correlated user event")?;
        let wake_at_ms = first_line_timestamp_matching(&lines, |value| {
            value.get("type").and_then(Value::as_str) == Some("event_msg")
                && value
                    .get("payload")
                    .and_then(|payload| payload.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "turn_started" | "task_started"))
        })
        .filter(|ts| *ts >= delivered_at_ms);
        let first_reaction_at_ms = first_line_timestamp_matching(&lines, |value| {
            value_contains_message_id(value, &fixture.message_id)
                && value.get("type").and_then(Value::as_str) == Some("response_item")
        })
        .filter(|ts| *ts >= delivered_at_ms);

        Ok(stage_from_parts(
            &fixture.message_id,
            &fixture.target,
            delivered_at_ms,
            wake_at_ms,
            first_reaction_at_ms,
        ))
    }
}

impl GrokAdapter {
    pub(crate) fn extract_fixture(fixture: &GrokFixture) -> Result<MessageStageEvidence> {
        let updates = read_jsonl(&fixture.updates_path)
            .with_context(|| format!("read Grok updates {}", fixture.updates_path.display()))?;
        let delivered_at_ms = first_line_timestamp_matching(&updates, |value| {
            value_contains_message_id(value, &fixture.message_id)
        })
        .context("Grok updates did not contain correlated probe message")?;
        let wake_at_ms = first_line_timestamp_matching(&updates, |value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "turn_started" | "task_started"))
        })
        .filter(|ts| *ts >= delivered_at_ms);
        let first_reaction_at_ms = first_line_timestamp_matching(&updates, |value| {
            value_contains_message_id(value, &fixture.message_id)
                && value
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "assistant_message" | "agent_message"))
        })
        .filter(|ts| *ts >= delivered_at_ms);

        let mut stage = stage_from_parts(
            &fixture.message_id,
            &fixture.target,
            delivered_at_ms,
            wake_at_ms,
            first_reaction_at_ms,
        );
        if let Some(turn_ended_at_ms) = fixture
            .events_path
            .as_ref()
            .filter(|path| path.exists())
            .map(|path| {
                first_grok_turn_ended_timestamp(path)
                    .with_context(|| format!("events_parse_failed: {}", path.display()))
            })
            .transpose()?
            .flatten()
        {
            stage.stage_statuses.push(StageStatusEvidence {
                stage: "turn_end".to_string(),
                status: "OBSERVED".to_string(),
                provenance: format!(
                    "Grok events artifact contains turn_ended at {turn_ended_at_ms}ms"
                ),
            });
            if stage.first_reaction_at_ms.is_none() {
                stage.reaction_status = Some("UNKNOWN".to_string());
            }
        }
        Ok(stage)
    }
}

fn stage_from_parts(
    message_id: &str,
    target: &str,
    delivered_at_ms: u64,
    wake_at_ms: Option<u64>,
    first_reaction_at_ms: Option<u64>,
) -> MessageStageEvidence {
    MessageStageEvidence {
        message_id: message_id.to_string(),
        target: target.to_string(),
        enqueued_at_ms: None,
        selected_at_ms: None,
        delivered_at_ms: Some(delivered_at_ms),
        wake_at_ms,
        first_reaction_at_ms,
        enqueued_status: "UNKNOWN",
        selected_status: "UNKNOWN",
        delivered_status: "OBSERVED",
        wake_status: if wake_at_ms.is_some() {
            "OBSERVED"
        } else {
            "UNKNOWN"
        },
        reaction_status: Some(
            if first_reaction_at_ms.is_some() {
                "OBSERVED"
            } else {
                "UNKNOWN"
            }
            .to_string(),
        ),
        stage_statuses: recorded_stage_statuses(delivered_at_ms, wake_at_ms, first_reaction_at_ms),
        terminal: "delivered",
    }
}

fn recorded_stage_statuses(
    delivered_at_ms: u64,
    wake_at_ms: Option<u64>,
    first_reaction_at_ms: Option<u64>,
) -> Vec<StageStatusEvidence> {
    let mut statuses = vec![
        StageStatusEvidence {
            stage: "enqueued".to_string(),
            status: "UNKNOWN".to_string(),
            provenance: "recorded adapter artifacts do not contain Cassy enqueue evidence"
                .to_string(),
        },
        StageStatusEvidence {
            stage: "selected".to_string(),
            status: "UNKNOWN".to_string(),
            provenance: "recorded adapter artifacts do not contain queue selection evidence"
                .to_string(),
        },
        StageStatusEvidence {
            stage: "delivered".to_string(),
            status: "OBSERVED".to_string(),
            provenance: format!(
                "recorded adapter artifact contains correlated delivery at {delivered_at_ms}ms"
            ),
        },
    ];
    statuses.push(match wake_at_ms {
        Some(ts) => StageStatusEvidence {
            stage: "wake".to_string(),
            status: "OBSERVED".to_string(),
            provenance: format!("recorded adapter artifact contains correlated wake at {ts}ms"),
        },
        None => StageStatusEvidence {
            stage: "wake".to_string(),
            status: "UNKNOWN".to_string(),
            provenance: "recorded adapter artifact does not prove worker wake".to_string(),
        },
    });
    statuses.push(match first_reaction_at_ms {
        Some(ts) => StageStatusEvidence {
            stage: "reaction".to_string(),
            status: "OBSERVED".to_string(),
            provenance: format!(
                "recorded adapter artifact contains correlated first reaction at {ts}ms"
            ),
        },
        None => StageStatusEvidence {
            stage: "reaction".to_string(),
            status: "UNKNOWN".to_string(),
            provenance: "recorded adapter artifact does not contain a correlated reaction"
                .to_string(),
        },
    });
    statuses
}

fn read_jsonl(path: &std::path::Path) -> Result<Vec<Value>> {
    let data = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in data.lines().filter(|line| !line.trim().is_empty()) {
        out.push(serde_json::from_str(line).with_context(|| format!("parse JSONL line: {line}"))?);
    }
    Ok(out)
}

fn first_correlated_jsonl_timestamp(
    path: &std::path::Path,
    message_id: &str,
) -> Result<Option<u64>> {
    let lines = read_jsonl(path)?;
    Ok(first_line_timestamp_matching(&lines, |value| {
        value_contains_message_id(value, message_id)
    }))
}

fn first_grok_turn_ended_timestamp(path: &std::path::Path) -> Result<Option<u64>> {
    let lines = read_jsonl(path)?;
    Ok(first_line_timestamp_matching(&lines, |value| {
        value.get("type").and_then(Value::as_str) == Some("turn_ended")
    })
    .or_else(|| {
        lines.iter().find_map(|value| {
            if value.get("type").and_then(Value::as_str) == Some("turn_ended") {
                value
                    .get("ts")
                    .and_then(Value::as_str)
                    .and_then(timestamp_ms)
            } else {
                None
            }
        })
    }))
}

fn first_line_timestamp_matching(
    lines: &[Value],
    predicate: impl Fn(&Value) -> bool,
) -> Option<u64> {
    lines
        .iter()
        .filter(|value| predicate(value))
        .filter_map(json_timestamp_ms)
        .min()
}

fn json_timestamp_ms(value: &Value) -> Option<u64> {
    value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .and_then(Value::as_str)
        .and_then(timestamp_ms)
}

fn timestamp_ms(raw: &str) -> Option<u64> {
    let ts = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
    u64::try_from(ts.timestamp_millis()).ok()
}

fn value_contains_message_id(value: &Value, message_id: &str) -> bool {
    match value {
        Value::String(s) => s.contains(message_id),
        Value::Array(items) => items
            .iter()
            .any(|item| value_contains_message_id(item, message_id)),
        Value::Object(map) => map
            .values()
            .any(|item| value_contains_message_id(item, message_id)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_jsonl(path: &std::path::Path, values: &[serde_json::Value]) {
        let body = values
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, format!("{body}\n")).unwrap();
    }

    #[test]
    fn claude_fixture_extracts_inbox_delivery_and_transcript_reaction() {
        let temp = tempfile::tempdir().unwrap();
        let inbox = temp.path().join("worker-a.json");
        let transcript = temp.path().join("claude-session.jsonl");
        std::fs::write(
            &inbox,
            serde_json::to_string_pretty(&json!([
                {
                    "from": "supervisor",
                    "text": "probe-message-id=claude-1 please reply",
                    "summary": "probe claude-1",
                    "timestamp": "2026-07-21T17:00:01.000Z",
                    "color": "green",
                    "read": false
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        write_jsonl(
            &transcript,
            &[
                json!({"timestamp":"2026-07-21T17:00:04.000Z","type":"assistant","message":"ack probe-message-id=claude-1"}),
            ],
        );

        let got = ClaudeAdapter::extract_fixture(&ClaudeFixture {
            inbox_path: inbox,
            transcript_path: Some(transcript),
            message_id: "claude-1".to_string(),
            target: "worker-a".to_string(),
        })
        .expect("claude fixture should parse");

        assert_eq!(got.message_id, "claude-1");
        assert_eq!(got.target, "worker-a");
        let got_json = serde_json::to_value(&got).unwrap();
        assert!(
            got_json["enqueued_at_ms"].is_null(),
            "recorded adapters must not fabricate enqueue timestamps: {got_json}"
        );
        assert!(
            got_json["selected_at_ms"].is_null(),
            "recorded adapters must not fabricate selection timestamps: {got_json}"
        );
        assert_eq!(got.delivered_at_ms, Some(1_784_653_201_000));
        assert_eq!(
            got.wake_at_ms, None,
            "Claude inbox persistence is delivery evidence, not worker wake evidence"
        );
        assert_eq!(got.first_reaction_at_ms, Some(1_784_653_204_000));
        assert_eq!(got.reaction_status.as_deref(), Some("OBSERVED"));
        assert_eq!(got.terminal, "delivered");
    }

    #[test]
    fn timestamp_ms_preserves_full_rfc3339_epoch_across_day_boundary() {
        assert_eq!(
            timestamp_ms("2026-07-21T23:59:59.999Z"),
            Some(1_784_678_399_999)
        );
        assert_eq!(
            timestamp_ms("2026-07-22T00:00:00.001Z"),
            Some(1_784_678_400_001)
        );
    }

    #[test]
    fn codex_fixture_correlates_rollout_turn_and_reaction() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        write_jsonl(
            &rollout,
            &[
                json!({"timestamp":"2026-07-21T17:00:00.000Z","type":"session_meta","payload":{"cwd":"/tmp/probe"}}),
                json!({"timestamp":"2026-07-21T17:00:02.000Z","type":"event_msg","payload":{"type":"user_message","message":"probe-message-id=codex-1"}}),
                json!({"timestamp":"2026-07-21T17:00:03.000Z","type":"event_msg","payload":{"type":"turn_started"}}),
                json!({"timestamp":"2026-07-21T17:00:06.000Z","type":"response_item","payload":{"type":"message","content":"ack probe-message-id=codex-1"}}),
            ],
        );

        let got = CodexAdapter::extract_fixture(&CodexFixture {
            rollout_path: rollout,
            message_id: "codex-1".to_string(),
            target: "worker-a".to_string(),
        })
        .expect("codex fixture should parse");

        assert_eq!(got.delivered_at_ms, Some(1_784_653_202_000));
        assert_eq!(got.wake_at_ms, Some(1_784_653_203_000));
        assert_eq!(got.first_reaction_at_ms, Some(1_784_653_206_000));
        assert_eq!(got.reaction_status.as_deref(), Some("OBSERVED"));
    }

    #[test]
    fn grok_fixture_uses_updates_and_events_without_inventing_reaction() {
        let temp = tempfile::tempdir().unwrap();
        let updates = temp.path().join("updates.jsonl");
        let events = temp.path().join("events.jsonl");
        write_jsonl(
            &updates,
            &[
                json!({"timestamp":"2026-07-21T17:00:02.000Z","type":"user_message","text":"probe-message-id=grok-1"}),
                json!({"timestamp":"2026-07-21T17:00:03.000Z","type":"turn_started"}),
            ],
        );
        write_jsonl(
            &events,
            &[json!({"ts":"2026-07-21T17:00:08.000Z","type":"turn_ended","outcome":"completed"})],
        );

        let got = GrokAdapter::extract_fixture(&GrokFixture {
            updates_path: updates,
            events_path: Some(events),
            message_id: "grok-1".to_string(),
            target: "worker-a".to_string(),
        })
        .expect("grok fixture should parse");

        assert_eq!(got.delivered_at_ms, Some(1_784_653_202_000));
        assert_eq!(got.wake_at_ms, Some(1_784_653_203_000));
        assert_eq!(got.first_reaction_at_ms, None);
        assert_eq!(got.reaction_status.as_deref(), Some("UNKNOWN"));
        assert_eq!(got.terminal, "delivered");
    }
}
