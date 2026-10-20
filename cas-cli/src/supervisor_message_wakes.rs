//! Directed supervisor messages use the existing queue selection, claims and transport.
use crate::ui::factory::daemon::runtime::{
    prompt_queue_jobs::job_thread_store, queue_and_events::RowFacts,
};
use cas_store::{PromptQueueStore, QueueOrigin, QueuedPrompt};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};

pub(crate) const QUIET: &str = "<cas-message-quiet/>";

/// Conservative acknowledgement vocabulary. Any substantive word or question
/// retains the wake. Replies are not acknowledgements merely because they reply.
pub(crate) fn pure_ack(text: &str) -> bool {
    if text.contains('?') {
        return false;
    }
    let text = text.trim();
    let text = text
        .strip_prefix("[CAS reply:")
        .and_then(|s| s.split_once('\n').map(|(_, s)| s))
        .unwrap_or(text);
    let text = text.split("<cas-worker-action ").next().unwrap_or(text);
    let words: Vec<_> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    !words.is_empty()
        && words.len() <= 8
        && words.iter().all(|word| {
            matches!(
                word.to_ascii_lowercase().as_str(),
                "ack"
                    | "acknowledged"
                    | "thanks"
                    | "thank"
                    | "you"
                    | "received"
                    | "got"
                    | "it"
                    | "ok"
                    | "okay"
                    | "noted"
                    | "understood"
            )
        })
}

pub(crate) fn quiet(text: &str) -> bool {
    text.starts_with(QUIET) || pure_ack(text)
}

/// Identity stays on the authenticated queue origin, never the sender's source label.
fn sender(row: &QueuedPrompt, supervisor: &str, agents: &[cas_types::Agent]) -> Option<String> {
    if crate::mcp::tools::service::agent_search_system::message::parse_peer_copy(&row.prompt)
        .is_some()
    {
        return None;
    }
    if row.urgent || !matches!(row.target.as_str(), "supervisor") && row.target != supervisor {
        return None;
    }
    match row.origin.as_ref()? {
        QueueOrigin::RegisteredAgent { agent_id } => agents
            .iter()
            .find(|agent| agent.id == *agent_id && !agent.name.eq_ignore_ascii_case(supervisor))
            .filter(|agent| {
                matches!(
                    agent.role,
                    cas_types::AgentRole::Supervisor
                        | cas_types::AgentRole::Worker
                        | cas_types::AgentRole::Director
                )
            })
            .map(|agent| agent.id.clone()),
        QueueOrigin::Daemon => crate::cloud::peer_mailbox::parse_envelope(&row.prompt)
            .map(|peer| format!("cloud:{}:{}", peer.repo, peer.sender_agent_id)),
        _ => None,
    }
}

fn wants_wake(row: &QueuedPrompt) -> bool {
    if let Some(peer) = crate::cloud::peer_mailbox::parse_envelope(&row.prompt) {
        peer.wakes_idle()
    } else {
        !quiet(&row.prompt)
    }
}

fn db(cas: &Path) -> anyhow::Result<Arc<Mutex<Connection>>> {
    job_thread_store("supervisor_message_wakes", cas, |cas| {
        let connection = Connection::open(cas.join("cas.db"))?;
        connection.busy_timeout(std::time::Duration::from_millis(50))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS supervisor_message_wakes (
            lead_id INTEGER PRIMARY KEY, recipient TEXT NOT NULL, created_at TEXT NOT NULL,
            injected_at TEXT NOT NULL, started_at TEXT, input_tokens INTEGER, output_tokens INTEGER,
            usage_evidence TEXT);
            CREATE INDEX IF NOT EXISTS supervisor_message_wakes_recipient ON supervisor_message_wakes(recipient,injected_at);
            CREATE TABLE IF NOT EXISTS supervisor_message_wake_pairs (
                lead_id INTEGER NOT NULL, sender TEXT NOT NULL, recipient TEXT NOT NULL,
                PRIMARY KEY(lead_id,sender));
            CREATE INDEX IF NOT EXISTS supervisor_message_wake_pair ON supervisor_message_wake_pairs(sender,recipient,lead_id);")?;
        Ok(Arc::new(Mutex::new(connection)))
    })
}

/// One durable row per submitted turn, even if many messages travelled in it.
/// Injection proves an attempted wake; transcript matching separately proves a
/// started turn and its usage. Retrying the same handoff never consumes the cap twice.
pub(crate) fn record(
    cas: &Path,
    lead: i64,
    recipient: &str,
    senders: &[String],
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let db = db(cas)?;
    let mut conn = db
        .lock()
        .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
    let tx = conn.transaction()?;
    tx.execute("INSERT OR IGNORE INTO supervisor_message_wakes(lead_id,recipient,created_at,injected_at) VALUES (?1,?2,?3,?4)",
        params![lead,recipient,created_at.to_rfc3339(),now.to_rfc3339()])?;
    for sender in senders {
        tx.execute("INSERT OR IGNORE INTO supervisor_message_wake_pairs(lead_id,sender,recipient) VALUES (?1,?2,?3)", params![lead,sender,recipient])?;
    }
    tx.commit()?;
    Ok(())
}

/// QueueSelect runs on the store job lane. Only ordinary directed messages are
/// batched here; envelopes requiring per-task revalidation retain their own row.
/// Such action events still use the existing wake slot and pane safety gate.
pub(crate) fn schedule(
    cas: &Path,
    session: &str,
    supervisor: &str,
    agents: &[cas_types::Agent],
    rows: &mut Vec<QueuedPrompt>,
    facts: &mut HashMap<i64, RowFacts>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let candidates: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let sender = sender(row, supervisor, agents)?;
            let fact = facts.get(&row.id)?;
            // Do not move another task's stale revalidation behind a batch leader.
            (fact.assignment_stale.is_none()
                && fact.lifecycle_task.is_none()
                && fact.merge_request.is_none()
                && !row.prompt.contains("<cas-blocker ")
                && !row.prompt.contains("<cas-verification-dispatch ")
                && !row.prompt.contains("<cas-qa-dispatch "))
            .then(|| (row.clone(), sender))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let config = crate::config::Config::load(cas)?.factory();
    let debounce =
        chrono::Duration::seconds(config.supervisor_message_debounce_secs.min(300) as i64);
    let cap = config.supervisor_message_wakes_per_hour.max(1);
    let db = db(cas)?;
    let conn = db
        .lock()
        .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
    let mut capped = HashMap::new();
    for (_, sender) in &candidates {
        if capped.contains_key(sender) {
            continue;
        }
        let count: u64 = conn.query_row("SELECT COUNT(*) FROM supervisor_message_wake_pairs p JOIN supervisor_message_wakes w USING(lead_id)
            WHERE p.sender=?1 AND p.recipient=?2 AND w.injected_at>?3",
            params![sender, supervisor, (now-chrono::Duration::hours(1)).to_rfc3339()], |row| row.get(0))?;
        capped.insert(sender.clone(), count >= cap);
    }
    drop(conn);
    let queue =
        crate::ui::factory::daemon::runtime::prompt_queue_jobs::job_thread_prompt_queue(cas)?;
    let wakers: Vec<_> = candidates
        .iter()
        .filter(|(row, sender)| wants_wake(row) && !capped[sender])
        .collect();
    let oldest = wakers.iter().map(|(row, _)| row.created_at).min();
    let due = oldest.is_some_and(|created| now >= created + debounce);
    let ids: HashSet<_> = candidates.iter().map(|(row, _)| row.id).collect();
    rows.retain(|row| !ids.contains(&row.id));
    if !due {
        for (row, sender) in &candidates {
            let (detail, recheck) = if capped[sender] && wants_wake(row) {
                let notice = format!(
                    "Wake cap: messages from {} to {supervisor} exceeded {cap} wakes/hour. Batched into this turn; read the pending messages before replying.",
                    row.source
                );
                queue.enqueue_idempotent(
                    "supervisor-wake-cap",
                    &row.target,
                    &notice,
                    Some(session),
                    Some("Supervisor message wake cap"),
                    None,
                    &format!(
                        "supervisor-wake-cap:{sender}:{supervisor}:{}",
                        now.timestamp() / 3600
                    ),
                    Some(&QueueOrigin::Daemon),
                )?;
                (
                    "directed supervisor message held by sender-pair wake cap",
                    now + chrono::Duration::minutes(1),
                )
            } else if wants_wake(row) {
                (
                    "directed supervisor message burst debounce",
                    oldest.unwrap_or(row.created_at) + debounce,
                )
            } else {
                (
                    "quiet acknowledgement or wake=false; readable in the next turn",
                    now + chrono::Duration::hours(1),
                )
            };
            queue.park_for_busy_recipient(row.id, Some(detail), recheck)?;
        }
        return Ok(());
    }
    // A capped sender or quiet acknowledgement may ride in an already-needed
    // turn for free; it cannot cause one. Every row keeps its own transport claim.
    let lead_id = wakers
        .iter()
        .min_by_key(|(row, _)| (row.created_at, row.id))
        .unwrap()
        .0
        .id;
    let mut lead = None;
    let mut passengers = Vec::new();
    for (row, _) in &candidates {
        if row.id == lead_id {
            lead = Some(row.clone());
        } else {
            passengers.push(row.clone());
        }
    }
    let fact = facts.get_mut(&lead_id).unwrap();
    fact.message_batch = passengers;
    fact.message_senders = wakers
        .into_iter()
        .map(|(_, sender)| sender.clone())
        .collect();
    fact.message_senders.sort();
    fact.message_senders.dedup();
    rows.push(lead.unwrap());
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Usage {
    input: u64,
    output: u64,
    started: Option<DateTime<Utc>>,
    complete: bool,
}

/// Bounded tail, read once per status call; unknown/truncated records stay
/// unmeasured. Claude streaming copies share message.id and count once.
fn transcript_usage(path: &Path, ids: &HashSet<i64>) -> anyhow::Result<HashMap<i64, Usage>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let offset = size.saturating_sub(8 * 1024 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    if offset > 0 {
        let mut partial = String::new();
        reader.read_line(&mut partial)?;
    }
    let mut result: HashMap<i64, Usage> = HashMap::new();
    let mut current = None;
    let mut cumulative: Option<(u64, u64)> = (offset == 0).then_some((0, 0));
    let mut baseline = None;
    let mut claude_messages: HashMap<String, (u64, u64)> = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let at = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let payload = value.get("payload");
        let payload_kind = payload
            .and_then(|p| p.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let user_content = if kind == "user" {
            value.pointer("/message/content")
        } else if kind == "response_item"
            && value.pointer("/payload/role").and_then(|v| v.as_str()) == Some("user")
        {
            value.pointer("/payload/content")
        } else {
            None
        };
        if let Some(content) = user_content {
            let texts: Vec<_> = content
                .as_str()
                .into_iter()
                .chain(content.as_array().into_iter().flatten().filter_map(|part| {
                    matches!(
                        part.get("type").and_then(|v| v.as_str()),
                        Some("text" | "input_text")
                    )
                    .then(|| part.get("text").and_then(|v| v.as_str()))
                    .flatten()
                }))
                .collect();
            // Tool-result user records do not delimit the turn.
            if !texts.is_empty() {
                current = ids
                    .iter()
                    .filter(|id| !result.contains_key(id))
                    .find(|id| {
                        texts
                            .iter()
                            .any(|text| text.contains(&format!("[cas #{id} ")))
                    })
                    .copied();
                baseline = cumulative;
                claude_messages.clear();
                if let Some(id) = current {
                    result.entry(id).or_default().started = at;
                }
            }
        }
        if payload_kind == "token_count" {
            if let Some(total) = value.pointer("/payload/info/total_token_usage") {
                let next = (
                    total.get("input_tokens").and_then(|v| v.as_u64()),
                    total.get("output_tokens").and_then(|v| v.as_u64()),
                );
                if let (Some(input), Some(output)) = next {
                    cumulative = Some((input, output));
                    if let (Some(id), Some((before_input, before_output))) = (current, baseline) {
                        if input >= before_input && output >= before_output {
                            let usage = result.entry(id).or_default();
                            usage.input = input - before_input;
                            usage.output = output - before_output;
                        }
                    }
                }
            }
        }
        if kind == "assistant"
            && let (Some(id), Some(usage), Some(message_id)) = (
                current,
                value.pointer("/message/usage"),
                value.pointer("/message/id").and_then(|v| v.as_str()),
            )
        {
            if let (Some(input), Some(output)) = (
                usage.get("input_tokens").and_then(|v| v.as_u64()),
                usage.get("output_tokens").and_then(|v| v.as_u64()),
            ) {
                let input = input
                    + usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                    + usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                claude_messages.insert(message_id.into(), (input, output));
                let measured = result.entry(id).or_default();
                measured.input = claude_messages.values().map(|(i, _)| i).sum();
                measured.output = claude_messages.values().map(|(_, o)| o).sum();
            }
        }
        if let Some(id) = current
            && (matches!(payload_kind, "task_complete" | "turn_completed")
                || kind == "system"
                    && value.get("subtype").and_then(|v| v.as_str()) == Some("turn_duration")
                || value
                    .pointer("/message/stop_reason")
                    .and_then(|v| v.as_str())
                    == Some("end_turn"))
        {
            let usage = result.entry(id).or_default();
            usage.complete = usage.input > 0 || usage.output > 0;
        }
    }
    Ok(result)
}

/// Read-only diagnostic work is charged to the requesting MCP call, never the
/// 100ms daemon loop. Persist completed measurements so unchanged turns are not
/// scanned again. A missing harness artifact is explicitly unmeasured.
pub(crate) fn report(cas: &Path, recipient: &str) -> anyhow::Result<String> {
    let db = db(cas)?;
    let pending: HashSet<i64> = {
        let conn = db
            .lock()
            .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
        let mut query=conn.prepare("SELECT lead_id FROM supervisor_message_wakes WHERE recipient=?1 AND input_tokens IS NULL ORDER BY injected_at DESC LIMIT 64")?;
        query
            .query_map([recipient], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };
    if !pending.is_empty() {
        let agents = crate::store::open_agent_store(cas)?.list(None)?;
        if let Some(agent) =
            crate::cli::factory::wedged::preferred_worker_registration(&agents, recipient)
            && let Some(path) =
                crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(
                    cas, agent,
                )
            && let Ok(measurements) = transcript_usage(&path, &pending)
        {
            let conn = db
                .lock()
                .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
            for (id, usage) in measurements {
                if usage.complete {
                    conn.execute("UPDATE supervisor_message_wakes SET started_at=?1,input_tokens=?2,output_tokens=?3,usage_evidence=?4 WHERE lead_id=?5 AND input_tokens IS NULL",
                        params![usage.started.map(|at|at.to_rfc3339()), usage.input,usage.output,path.display().to_string(),id])?;
                }
            }
        }
    }
    let conn = db
        .lock()
        .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
    let since = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let wakes: u64 = conn.query_row(
        "SELECT COUNT(*) FROM supervisor_message_wakes WHERE recipient=?1 AND injected_at>?2",
        params![recipient, since],
        |row| row.get(0),
    )?;
    let mut query=conn.prepare("SELECT input_tokens+output_tokens FROM supervisor_message_wakes WHERE recipient=?1 AND injected_at>?2 AND input_tokens IS NOT NULL ORDER BY input_tokens+output_tokens")?;
    let totals: Vec<u64> = query
        .query_map(params![recipient, since], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let median = if totals.is_empty() {
        "unmeasured".into()
    } else {
        let mid = totals.len() / 2;
        if totals.len() % 2 == 0 {
            ((totals[mid - 1] + totals[mid]) / 2).to_string()
        } else {
            totals[mid].to_string()
        }
    };
    let latency: Option<f64>=conn.query_row("SELECT MAX((julianday(started_at)-julianday(created_at))*86400000) FROM supervisor_message_wakes WHERE recipient=?1 AND injected_at>?2",params![recipient,since],|row|row.get(0))?;
    Ok(format!(
        "directed_message_wakes_last_hour: {wakes}; median_tokens_per_woken_turn: {median}; measured_turns: {}; max_observed_start_latency_ms: {}\n",
        totals.len(),
        latency
            .map(|n| format!("{n:.0}"))
            .unwrap_or_else(|| "unmeasured".into())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::factory::daemon::runtime::prompt_queue_jobs::QueueSelectRequest;
    use crate::ui::factory::daemon::runtime::queue_and_events::run_queue_select;
    use cas_store::QueueOrigin;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, cas_types::Agent) {
        let root = tempfile::tempdir().unwrap();
        let cas = crate::store::init_cas_dir(root.path()).unwrap();
        let mut peer = cas_types::Agent::new_with_role(
            "peer-id".into(),
            "peer".into(),
            cas_types::AgentRole::Supervisor,
        );
        peer.factory_session = Some("other-session".into());
        crate::store::open_agent_store(&cas)
            .unwrap()
            .register(&peer)
            .unwrap();
        (root, cas, peer)
    }
    fn enqueue(cas: &Path, peer: &cas_types::Agent, body: &str) -> i64 {
        crate::store::open_prompt_queue_store(cas)
            .unwrap()
            .enqueue_urgent_with_outcome(
                &peer.name,
                "supervisor",
                body,
                Some("wake-session"),
                Some("field report"),
                Some(cas_store::NotificationPriority::Normal),
                false,
                Some(&QueueOrigin::RegisteredAgent {
                    agent_id: peer.id.clone(),
                }),
            )
            .unwrap()
            .id()
    }
    fn select(
        cas: &Path,
    ) -> crate::ui::factory::daemon::runtime::prompt_queue_jobs::QueueSelection {
        run_queue_select(
            &QueueSelectRequest {
                cas_dir: cas.into(),
                session_name: "wake-session".into(),
                supervisor_name: "wake-sup".into(),
                worker_names: vec![],
                dead_workers: Default::default(),
                limit: 10,
                operator_replies: false,
                deferred_operator_replies: Default::default(),
                adopt_operator_notices: false,
            },
            1,
        )
        .unwrap()
    }
    fn age(cas: &Path) {
        rusqlite::Connection::open(cas.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at=?1,next_attempt_at=?1",
                [(Utc::now() - chrono::Duration::seconds(21)).to_rfc3339()],
            )
            .unwrap();
    }
    #[test]
    fn cas_1e7e2_acknowledgements_wait_readable_for_the_next_turn() {
        let (_root, cas, peer) = setup();
        let ack = enqueue(&cas, &peer, "thanks, received");
        let receipt = enqueue(&cas, &peer, "<cas-message-quiet/>\nAcknowledged.");
        age(&cas);
        assert!(
            select(&cas).rows.is_empty(),
            "acknowledgements must never start a turn"
        );
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let readable = queue
            .surface_unseen_for_recipient("supervisor", Some("wake-session"), 20)
            .unwrap();
        assert_eq!(
            readable.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ack, receipt]
        );
    }
    #[test]
    fn cas_1e7e2_pair_cap_holds_messages_and_surfaces_a_notice() {
        let (_root, cas, peer) = setup();
        let now = Utc::now();
        for lead in 1..=6 {
            record(&cas, -lead, "wake-sup", &[peer.id.clone()], now, now).unwrap();
        }
        let capped = enqueue(&cas, &peer, "Additional field report requiring attention");
        age(&cas);
        assert!(select(&cas).rows.is_empty(), "seventh wake must be held");
        // A separate sender has its own budget, so this is a pair cap.
        let mut other = peer.clone();
        other.id = "second-peer".into();
        other.name = "second".into();
        crate::store::open_agent_store(&cas)
            .unwrap()
            .register(&other)
            .unwrap();
        let allowed = enqueue(&cas, &other, "Independent peer field report");
        age(&cas);
        let selected = select(&cas);
        assert!(selected.rows.iter().any(|row| row.id == allowed));
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let readable = queue
            .surface_unseen_for_recipient("supervisor", Some("wake-session"), 20)
            .unwrap();
        assert!(
            readable.iter().any(|row| row.id == capped),
            "cap retains the message for the next turn"
        );
        assert_eq!(
            readable
                .iter()
                .filter(|row| row.prompt.starts_with("Wake cap:"))
                .count(),
            1,
            "one readable notice, no notice storm"
        );
    }

    #[test]
    fn cas_1e7e2_token_cost_uses_only_the_matched_completed_harness_turn() {
        let dir = tempfile::tempdir().unwrap();
        for (name, records, expected) in [
            (
                "claude",
                serde_json::json!([
                    {"type":"user","timestamp":"2026-10-20T17:00:00Z","message":{"content":"[cas #42 peer] field report"}},
                    {"type":"assistant","message":{"id":"a","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":5}}},
                    {"type":"user","message":{"content":[{"type":"tool_result","content":"tool response"}]}},
                    {"type":"assistant","message":{"id":"a","stop_reason":"end_turn","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":9}}},
                    {"type":"user","message":{"content":"Operator unrelated next turn"}},
                    {"type":"assistant","message":{"id":"b","stop_reason":"end_turn","usage":{"input_tokens":999,"output_tokens":999}}}
                ]),
                (100, 9),
            ),
            (
                "codex",
                serde_json::json!([
                    {"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"output_tokens":100}}}},
                    {"type":"response_item","timestamp":"2026-10-20T17:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"[cas #42 peer] field report"}]}},
                    {"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":700,"output_tokens":140}}}},
                    {"type":"event_msg","payload":{"type":"task_complete"}},
                    {"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"unrelated"}]}},
                    {"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":999,"output_tokens":999}}}}
                ]),
                (200, 40),
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(
                &path,
                records
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r.to_string() + "\n")
                    .collect::<String>(),
            )
            .unwrap();
            let usage = transcript_usage(&path, &HashSet::from([42])).unwrap();
            assert!(usage[&42].complete, "{name}");
            assert_eq!((usage[&42].input, usage[&42].output), expected, "{name}");
            assert_eq!(
                usage[&42].started,
                Some(
                    DateTime::parse_from_rfc3339("2026-10-20T17:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc)
                )
            );
        }
    }
}
