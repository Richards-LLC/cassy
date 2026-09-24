//! Mirror completed supervisor final answers into the durable Commander lane.
//! The daemon observes both Claude and Codex transcripts; Codex has no Stop
//! hook, so a hook-only mirror would silently omit its supervisor turns.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use cas_store::PromptQueueStore;

const TRANSCRIPT_TAIL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REPLY_CHARS: usize = 4_000;

#[derive(Debug)]
struct CompletedTurn {
    key: String,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    text: String,
    commander_id: Option<i64>,
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn text_blocks(value: &Value, path: &str, block_type: &str) -> Option<String> {
    let text = value
        .pointer(path)?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some(block_type))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// The provenance marker is only a hint. The queue row is independently
/// checked for a hub-stamped paired-device origin before using its reply_to.
fn commander_marker(text: &str) -> Option<i64> {
    text.match_indices("[cas #").find_map(|(offset, _)| {
        let start = offset + 6;
        let digits = text[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        text[start + digits.len()..]
            .starts_with(" operator ")
            .then(|| digits.parse::<i64>().ok())
            .flatten()
    })
}

fn bounded_reply(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_REPLY_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_REPLY_CHARS - 2).collect();
    let cut = cut.rsplit_once('\n').map(|(head, _)| head).unwrap_or(&cut);
    format!("{cut}\n…")
}

fn transcript_tail(path: &Path) -> std::io::Result<Vec<Value>> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TRANSCRIPT_TAIL_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .lines()
        .skip(usize::from(start > 0))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn completed_turns(values: &[Value], cli: cas_mux::SupervisorCli) -> Vec<CompletedTurn> {
    let mut turns = Vec::new();
    let mut started: Option<(String, DateTime<Utc>, Option<i64>)> = None;
    let mut final_text: Option<String> = None;
    for value in values {
        let Some(at) = timestamp(value) else { continue };
        match cli {
            cas_mux::SupervisorCli::Claude => {
                if value.get("type").and_then(Value::as_str) == Some("user")
                    && value.get("isMeta").and_then(Value::as_bool) != Some(true)
                    && value.get("isSidechain").and_then(Value::as_bool) != Some(true)
                    && let Some(prompt) = value.pointer("/message/content").and_then(Value::as_str)
                {
                    let key = value
                        .get("promptId")
                        .or_else(|| value.get("uuid"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    if !key.is_empty() {
                        started = Some((key, at, commander_marker(prompt)));
                        final_text = None;
                    }
                }
                if value.get("type").and_then(Value::as_str) == Some("assistant")
                    && value.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
                    && value.get("isSidechain").and_then(Value::as_bool) != Some(true)
                {
                    if let Some(text) = text_blocks(value, "/message/content", "text") {
                        final_text = Some(text);
                    }
                    if value
                        .pointer("/message/stop_reason")
                        .and_then(Value::as_str)
                        == Some("end_turn")
                    {
                        if let (Some((key, begun, commander_id)), Some(text)) =
                            (started.take(), final_text.take())
                        {
                            turns.push(CompletedTurn {
                                key,
                                started_at: begun,
                                completed_at: at,
                                text,
                                commander_id,
                            });
                        }
                    }
                }
                if value.get("type").and_then(Value::as_str) == Some("system")
                    && value.get("subtype").and_then(Value::as_str) == Some("turn_duration")
                    && let (Some((key, begun, commander_id)), Some(text)) =
                        (started.take(), final_text.take())
                {
                    turns.push(CompletedTurn {
                        key,
                        started_at: begun,
                        completed_at: at,
                        text,
                        commander_id,
                    });
                }
            }
            cas_mux::SupervisorCli::Codex => {
                let outer = value.get("type").and_then(Value::as_str);
                let payload = value.get("payload");
                let kind = payload.and_then(|p| p.get("type")).and_then(Value::as_str);
                if outer == Some("event_msg")
                    && matches!(kind, Some("task_started" | "turn_started"))
                {
                    if let Some(id) = payload
                        .and_then(|p| p.get("turn_id"))
                        .and_then(Value::as_str)
                    {
                        started = Some((id.to_owned(), at, None));
                        final_text = None;
                    }
                }
                if outer == Some("response_item") && kind == Some("message") {
                    let role = payload.and_then(|p| p.get("role")).and_then(Value::as_str);
                    if role == Some("user") {
                        if let Some(prompt) = text_blocks(value, "/payload/content", "input_text") {
                            if let Some((_, _, commander_id)) = started.as_mut() {
                                *commander_id = commander_marker(&prompt).or(*commander_id);
                            }
                        }
                    } else if role == Some("assistant")
                        && payload.and_then(|p| p.get("phase")).and_then(Value::as_str)
                            == Some("final_answer")
                    {
                        final_text = text_blocks(value, "/payload/content", "output_text");
                    }
                }
                if outer == Some("event_msg")
                    && kind == Some("item_completed")
                    && value.pointer("/payload/item/type").and_then(Value::as_str)
                        == Some("AgentMessage")
                    && value.pointer("/payload/item/phase").and_then(Value::as_str)
                        == Some("final_answer")
                {
                    final_text = text_blocks(value, "/payload/item/content", "Text");
                }
                if outer == Some("event_msg")
                    && matches!(kind, Some("task_complete" | "turn_completed"))
                {
                    if let (Some((key, begun, commander_id)), Some(text)) =
                        (started.take(), final_text.take())
                    {
                        let completed_id = payload
                            .and_then(|p| p.get("turn_id"))
                            .and_then(Value::as_str);
                        if completed_id.is_none_or(|id| id == key) {
                            turns.push(CompletedTurn {
                                key,
                                started_at: begun,
                                completed_at: at,
                                text,
                                commander_id,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    turns
}

fn activation_time(
    root: &Path,
    session: &str,
    first_pair_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let directory = root.join("commander-mirror");
    std::fs::create_dir_all(&directory).ok()?;
    let hash = format!("{:x}", Sha256::digest(session.as_bytes()));
    let marker = directory.join(format!("{hash}.started"));
    // First rollout should pick up a Commander turn that completed just
    // before this poll, without flooding the phone with an old session's
    // transcript. Later daemon restarts reuse the durable start watermark.
    let initial = first_pair_at.max(Utc::now() - chrono::Duration::minutes(2));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        let _ = file.write_all(initial.to_rfc3339().as_bytes());
    }
    std::fs::read_to_string(marker)
        .ok()
        .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
        .map(|at| at.with_timezone(&Utc))
        .or(Some(initial))
}

pub(super) fn mirror_supervisor_replies(
    root: &Path,
    project_path: &Path,
    session: &str,
    queue: &dyn PromptQueueStore,
) {
    let Ok(Some(_paired_device)) = queue.latest_verified_operator_device(session) else {
        return;
    };
    let Ok(Some(first_pair_at)) = queue.first_verified_operator_at(session) else {
        return;
    };
    let Some(active_since) = activation_time(root, session, first_pair_at) else {
        return;
    };
    let Ok(agents) = crate::store::open_agent_store(root) else {
        return;
    };
    let Ok(agents) = agents.list(Some(cas_types::AgentStatus::Active)) else {
        return;
    };
    for agent in agents.iter().filter(|agent| {
        agent.role == cas_types::AgentRole::Supervisor
            && agent.factory_session.as_deref() == Some(session)
    }) {
        // Supervisor registration persists its own harness under
        // `supervisor_cli`; `worker_cli` names the default child harness.
        let cli = agent
            .metadata
            .get("supervisor_cli")
            .and_then(|value| value.parse::<cas_mux::SupervisorCli>().ok())
            .unwrap_or(cas_mux::SupervisorCli::Claude);
        let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
        let Some(path) =
            crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
                project_path.to_str(),
                session_id,
                cli,
                agent
                    .metadata
                    .get("supervisor_account_dir")
                    .map(String::as_str),
            )
        else {
            continue;
        };
        let Ok(values) = transcript_tail(&path) else {
            continue;
        };
        for turn in completed_turns(&values, cli)
            .into_iter()
            .filter(|turn| turn.completed_at >= active_since)
        {
            let prior = turn
                .commander_id
                .and_then(|id| queue.queued_prompt(id).ok().flatten())
                .filter(|row| {
                    row.factory_session.as_deref() == Some(session)
                        && row
                            .origin
                            .as_ref()
                            .and_then(cas_store::QueueOrigin::verified_device_id)
                            .is_some()
                        && row.source.starts_with("commander:")
                        && (row.target == "supervisor" || row.target == agent.name)
                });
            let device = prior
                .as_ref()
                .and_then(|row| {
                    row.origin
                        .as_ref()
                        .and_then(cas_store::QueueOrigin::verified_device_id)
                })
                .unwrap_or("*");
            let text = bounded_reply(
                &crate::hooks::handlers::handlers_events::message_display::redact_secrets(
                    turn.text,
                )
                .0,
            );
            if text.is_empty() {
                continue;
            }
            let summary: String = text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect();
            let kind = if prior.is_some() {
                crate::ui::factory::OperatorTurnKind::Answer
            } else {
                crate::ui::factory::OperatorTurnKind::Status
            };
            let payload = crate::ui::factory::OperatorReplyPayload {
                schema_version: 2,
                reply_to: prior.as_ref().map(|row| row.id),
                message: text,
                summary: summary.clone(),
                device_id: device.to_owned(),
                operator_label: None,
                kind,
                attachments: Vec::new(),
            };
            let Ok(payload) = serde_json::to_string(&payload) else {
                continue;
            };
            let key = format!("commander-mirror:{}:{}:{}", session, agent.id, turn.key);
            if let Err(error) = queue.mirror_supervisor_turn(
                session,
                &key,
                turn.started_at,
                turn.completed_at,
                &payload,
                &summary,
                device,
                kind.as_str(),
            ) {
                tracing::warn!(%error, "supervisor Commander mirror queue write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{AgentStore, OperatorStamp, SqliteAgentStore, SqlitePromptQueueStore};
    use cas_types::{Agent, AgentRole};

    fn codex_turn(prompt: &str) -> Vec<Value> {
        vec![
            serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:02Z","type":"response_item","payload":{"type":"function_call_output","output":"secret tool output"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:03Z","type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"**Ready**\n- Next step is clear."}]}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:04Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1"}}),
        ]
    }

    #[test]
    fn codex_final_excludes_tool_output_and_keeps_markdown_and_reply_id() {
        let turns = completed_turns(
            &codex_turn("[cas #42 operator Alice@phone verified 0s first] Hello"),
            cas_mux::SupervisorCli::Codex,
        );
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "**Ready**\n- Next step is clear.");
        assert_eq!(turns[0].commander_id, Some(42));
    }

    #[test]
    fn claude_only_mirrors_completed_final_text() {
        let values = vec![
            serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"user","uuid":"prompt-1","message":{"content":"status"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:01Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Working"},{"type":"tool_use","input":{"secret":"hidden"}}],"stop_reason":"tool_use"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:02Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:03Z","type":"system","subtype":"turn_duration"}),
        ];
        let turns = completed_turns(&values, cas_mux::SupervisorCli::Claude);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "Done");
    }

    #[test]
    fn mirrored_history_is_idempotent_and_explicit_reply_suppresses_it() {
        let temp = tempfile::tempdir().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let stamp = OperatorStamp {
            operator: "Alice".into(),
            device_id: "phone".into(),
            device_label: "phone".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        let message = queue
            .enqueue_operator_message(
                "commander:Alice@phone",
                "supervisor",
                "Hello",
                Some("factory-1"),
                None,
                None,
                false,
                None,
                &stamp,
            )
            .unwrap();
        let turn = completed_turns(
            &codex_turn(&format!(
                "[cas #{} operator Alice@phone verified 0s first] Hello",
                message.id()
            )),
            cas_mux::SupervisorCli::Codex,
        )
        .pop()
        .unwrap();
        let payload = serde_json::to_string(&crate::ui::factory::OperatorReplyPayload {
            schema_version: 2,
            reply_to: Some(message.id()),
            message: turn.text.clone(),
            summary: "Ready".into(),
            device_id: "phone".into(),
            operator_label: None,
            kind: crate::ui::factory::OperatorTurnKind::Answer,
            attachments: Vec::new(),
        })
        .unwrap();
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-1",
                    turn.started_at,
                    turn.completed_at,
                    &payload,
                    "Ready",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_some()
        );
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-1",
                    turn.started_at,
                    turn.completed_at,
                    &payload,
                    "Ready",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_none()
        );
        let history = queue
            .conversation_history("factory-1", "phone", None, 20)
            .unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|row| row.target == "operator")
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .find(|row| row.target == "operator")
                .unwrap()
                .prompt,
            payload
        );

        // A later turn already sent an explicit operator row while composing.
        let explicit_at = Utc::now();
        queue
            .enqueue_with_session("supervisor", "operator", "explicit", "factory-1")
            .unwrap();
        let end = explicit_at + chrono::Duration::seconds(2);
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-2",
                    explicit_at - chrono::Duration::seconds(1),
                    end,
                    &payload,
                    "Ready",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn no_verified_pair_does_not_create_a_mirror_marker_or_reply() {
        let temp = tempfile::tempdir().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        mirror_supervisor_replies(temp.path(), temp.path(), "factory-1", &queue);
        assert!(!temp.path().join("commander-mirror").exists());
        assert!(
            queue
                .conversation_history("factory-1", "phone", None, 20)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn daemon_scan_mirrors_supervisor_only_into_commander_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let queue = SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let stamp = OperatorStamp {
            operator: "Alice".into(),
            device_id: "phone".into(),
            device_label: "phone".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        queue
            .enqueue_operator_message(
                "commander:Alice@phone",
                "supervisor",
                "status",
                Some("factory-1"),
                None,
                None,
                false,
                None,
                &stamp,
            )
            .unwrap();
        // Pairing activates mirroring. A worker transcript with a final answer
        // must still produce no operator row.
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let agents = SqliteAgentStore::open(&root).unwrap();
        agents.init().unwrap();
        let account = temp.path().join("account");
        let sessions = account.join("sessions/2026/09/23");
        std::fs::create_dir_all(&sessions).unwrap();
        let rollout = sessions.join("rollout-2026-09-23T12-00-00-test.jsonl");
        let mut lines = vec![
            serde_json::json!({"type":"session_meta","payload":{"cwd":temp.path().to_str().unwrap(),"source":"cli"}}),
        ];
        let now = Utc::now() + chrono::Duration::seconds(1);
        for (index, mut value) in codex_turn("status").into_iter().enumerate() {
            value["timestamp"] = serde_json::Value::String(
                (now + chrono::Duration::seconds(index as i64)).to_rfc3339(),
            );
            lines.push(value);
        }
        std::fs::write(
            &rollout,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let mut worker =
            Agent::new_with_role("worker-id".into(), "worker".into(), AgentRole::Worker);
        worker.factory_session = Some("factory-1".into());
        worker.metadata.insert("worker_cli".into(), "codex".into());
        worker.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agents.register(&worker).unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        assert_eq!(
            queue
                .conversation_history("factory-1", "phone", None, 20)
                .unwrap()
                .iter()
                .filter(|row| row.target == "operator")
                .count(),
            0
        );

        let mut supervisor = Agent::new_with_role(
            "supervisor-id".into(),
            "supervisor".into(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-1".into());
        supervisor
            .metadata
            .insert("supervisor_cli".into(), "codex".into());
        supervisor.metadata.insert(
            "supervisor_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agents.register(&supervisor).unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let history = queue
            .conversation_history("factory-1", "phone", None, 20)
            .unwrap();
        let replies: Vec<_> = history
            .iter()
            .filter(|row| row.target == "operator")
            .collect();
        assert_eq!(replies.len(), 1);
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&replies[0].prompt).unwrap();
        assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Status);
        assert_eq!(payload.message, "**Ready**\n- Next step is clear.");
        assert_eq!(payload.device_id, "*");
    }

    #[test]
    fn concurrent_daemon_polls_create_one_reply() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let queue = SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let now = Utc::now();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let joins: Vec<_> = (0..8)
            .map(|_| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let queue = SqlitePromptQueueStore::open(&root).unwrap();
                    barrier.wait();
                    queue.mirror_supervisor_turn(
                        "factory-1", "same-turn", now - chrono::Duration::seconds(2), now,
                        r#"{"schema_version":2,"reply_to":null,"message":"done","summary":"done","device_id":"*","kind":"status","attachments":[]}"#,
                        "done", "*", "status",
                    ).unwrap()
                })
            })
            .collect();
        let created = joins
            .into_iter()
            .map(|join| join.join().unwrap())
            .filter(Option::is_some)
            .count();
        assert_eq!(created, 1);
        assert_eq!(
            queue
                .conversation_history("factory-1", "phone", None, 20)
                .unwrap()
                .len(),
            1
        );
    }
}
