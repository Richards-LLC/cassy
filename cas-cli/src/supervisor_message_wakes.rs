//! Directed supervisor messages use the existing queue selection, claims and transport.
use crate::ui::factory::daemon::runtime::{
    prompt_queue_jobs::job_thread_store, queue_and_events::RowFacts,
};
use cas_store::{PromptQueueStore, QueueOrigin, QueuedPrompt};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

pub(crate) const QUIET: &str = cas_store::QUIET_MESSAGE_MARKER;
const DEBOUNCE_DETAIL: &str = "directed supervisor message burst debounce";

// One warning per project per minute, shared across selection job threads.
// Expired entries are removed on the next failure so retired projects do not
// accumulate in a long-lived daemon. Suppression never changes delivery.
const FAILURE_LOG_INTERVAL: Duration = Duration::from_secs(60);
static FAILURE_WARNINGS: LazyLock<Mutex<HashMap<PathBuf, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn warn_batching_failure(cas: &Path, error: &anyhow::Error, now: Instant) {
    let emit = {
        let mut warnings = FAILURE_WARNINGS.lock().unwrap_or_else(|error| error.into_inner());
        warnings.retain(|_, last| now.saturating_duration_since(*last) < FAILURE_LOG_INTERVAL);
        if warnings.contains_key(cas) {
            false
        } else {
            warnings.insert(cas.to_path_buf(), now);
            true
        }
    };
    if emit {
        tracing::warn!(
            target: "cas::coordination", stage = "supervisor_message_batching_failed", %error,
            "supervisor message batching unavailable; continuing normal queue delivery"
        );
    }
}

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
    let text = text
        .strip_prefix("[CAS reply:")
        .and_then(|s| s.split_once('\n').map(|(_, text)| text))
        .unwrap_or(text);
    text.starts_with(QUIET) || pure_ack(text)
}

/// Identity stays on the authenticated queue origin, never the sender's source label.
fn sender(row: &QueuedPrompt, supervisor: &str, agents: &[cas_types::Agent]) -> Option<String> {
    sender_fields(&row.prompt, &row.target, row.urgent, row.origin.as_ref(), supervisor, agents)
}

fn sender_fields(
    prompt: &str, target: &str, urgent: bool, origin: Option<&QueueOrigin>,
    supervisor: &str, agents: &[cas_types::Agent],
) -> Option<String> {
    if crate::mcp::tools::service::agent_search_system::message::parse_peer_copy(prompt)
        .is_some()
    {
        return None;
    }
    if urgent || target != "supervisor" && target != supervisor {
        return None;
    }
    match origin? {
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
        QueueOrigin::Daemon => crate::cloud::peer_mailbox::parse_envelope(prompt)
            .map(|peer| format!("cloud:{}:{}", peer.repo, peer.sender_agent_id)),
        _ => None,
    }
}

fn wants_wake(row: &QueuedPrompt) -> bool {
    wants_wake_text(&row.prompt)
}

fn wants_wake_text(prompt: &str) -> bool {
    if let Some(peer) = crate::cloud::peer_mailbox::parse_envelope(prompt) {
        peer.wakes_idle()
    } else {
        !quiet(prompt)
    }
}

/// A daemon-local legacy hold can remove a passenger after QueueSelect.
/// Charge only the eligible sender pairs still represented in this handoff.
pub(crate) fn retain_batch_senders(
    senders: &mut Vec<String>, lead: &QueuedPrompt, passengers: &[QueuedPrompt],
    supervisor: &str, agents: &[cas_types::Agent],
) {
    senders.retain(|id| std::iter::once(lead).chain(passengers).any(|row| {
        wants_wake(row) && sender(row, supervisor, agents).as_ref() == Some(id)
    }));
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
        .filter_map(|row| sender(row, supervisor, agents).map(|sender| (row.clone(), sender)))
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let config = crate::config::Config::load(cas)?.factory();
    let debounce =
        chrono::Duration::seconds(config.supervisor_message_debounce_secs.min(300) as i64);
    let cap = config.supervisor_message_wakes_per_hour.max(1);
    let queue =
        crate::ui::factory::daemon::runtime::prompt_queue_jobs::job_thread_prompt_queue(cas)?;
    let db = db(cas)?;
    let conn = db
        .lock()
        .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
    let mut capped = HashMap::new();
    let mut pair_capped = |sender: &str| -> anyhow::Result<bool> {
        if let Some(capped) = capped.get(sender) {
            return Ok(*capped);
        }
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM supervisor_message_wake_pairs p JOIN supervisor_message_wakes w USING(lead_id)
            WHERE p.sender=?1 AND p.recipient=?2 AND w.injected_at>?3",
            params![sender, supervisor, (now-chrono::Duration::hours(1)).to_rfc3339()], |row| row.get(0))?;
        let held = count >= cap.min(i64::MAX as u64) as i64;
        capped.insert(sender.to_string(), held);
        Ok(held)
    };
    for (_, sender) in &candidates {
        pair_capped(sender)?;
    }
    // A parked leader is invisible to normal QueueSelect until its deadline.
    // New arrivals join that deadline, without draining/claiming the leader or
    // scanning other sessions. Validate origin and pair cap as for fresh rows.
    struct WaitingMessage {
        deadline: String,
        prompt: String,
        target: String,
        urgent: bool,
        origin: Option<QueueOrigin>,
    }
    let waiting: Vec<WaitingMessage> = conn.prepare(
        "SELECT q.next_attempt_at,q.prompt,q.target,q.urgent,q.origin_agent_id,q.origin_kind FROM prompt_queue q
         WHERE q.processed_at IS NULL AND q.acked_at IS NULL
           AND q.last_pending_detail=?1 AND q.next_attempt_at>?4
           AND (q.factory_session=?2 OR q.factory_session IS NULL)
           AND q.target IN ('supervisor',?3)
           AND NOT EXISTS (SELECT 1 FROM prompt_queue_recipient_seen seen
               WHERE seen.prompt_id=q.id AND seen.recipient=q.target)
         ORDER BY q.next_attempt_at,q.id LIMIT 64"
    )?.query_map(params![DEBOUNCE_DETAIL,session,supervisor,now.to_rfc3339()], |row| {
        let kind: Option<String> = row.get(5)?;
        Ok(WaitingMessage {
            deadline: row.get(0)?, prompt: row.get(1)?, target: row.get(2)?, urgent: row.get(3)?,
            origin: QueueOrigin::from_columns(row.get(4)?, kind.as_deref()),
        })
    })?.collect::<Result<_,_>>()?;
    let mut inherited_deadline: Option<DateTime<Utc>> = None;
    for row in waiting {
        // One malformed historical deadline must not disable healthy batching.
        let Ok(deadline) = DateTime::parse_from_rfc3339(&row.deadline) else { continue };
        let deadline = deadline.with_timezone(&Utc);
        let Some(sender) = sender_fields(&row.prompt, &row.target, row.urgent, row.origin.as_ref(), supervisor, agents) else { continue };
        if wants_wake_text(&row.prompt) && !pair_capped(&sender)? {
            inherited_deadline = Some(inherited_deadline.map_or(deadline, |prior| prior.min(deadline)));
        }
    }
    drop(conn);
    for (row, sender) in &candidates {
        if capped[sender] && wants_wake(row) {
            let notice = format!(
                "Wake cap: messages from {} to {supervisor} reached {cap} wakes/hour. Batched into this turn; read the pending messages before replying.",
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
        }
    }
    let wakers: Vec<_> = candidates
        .iter()
        .filter(|(row, sender)| wants_wake(row) && !capped[sender])
        .collect();
    let deadline = wakers.iter().map(|(row, _)| row.created_at + debounce)
        .chain(inherited_deadline).min();
    let due = deadline.is_some_and(|deadline| now >= deadline);
    let ids: HashSet<_> = candidates.iter().map(|(row, _)| row.id).collect();
    rows.retain(|row| !ids.contains(&row.id));
    if !due {
        for (row, sender) in &candidates {
            let (detail, recheck) = if capped[sender] && wants_wake(row) {
                (
                    "directed supervisor message held by sender-pair wake cap",
                    now + chrono::Duration::minutes(1),
                )
            } else if wants_wake(row) {
                (
                    DEBOUNCE_DETAIL,
                    deadline.unwrap_or(row.created_at + debounce),
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
    // A capped sender or acknowledgement may ride in an already-needed turn
    // for free. Task-bound envelopes retain separate rows and revalidation.
    let can_batch = |row: &QueuedPrompt| {
        facts.get(&row.id).is_some_and(|fact| {
            fact.assignment_stale.is_none()
                && fact.deferred_inbox.is_none()
                && fact.lifecycle_task.is_none()
                && fact.merge_request.is_none()
        }) && !row.prompt.contains("<cas-blocker ")
            && !row.prompt.contains("<cas-verification-dispatch ")
            && !row.prompt.contains("<cas-qa-dispatch ")
    };
    let batchable: HashSet<_> = candidates
        .iter()
        .filter(|(row, _)| can_batch(row))
        .map(|(row, _)| row.id)
        .collect();
    let batch_wakers: Vec<_> = wakers
        .iter()
        .filter(|(row, _)| batchable.contains(&row.id))
        .collect();
    let lead_id = batch_wakers
        .iter()
        .min_by_key(|(row, _)| (row.created_at, row.id))
        .map(|(row, _)| row.id);
    let mut passengers = Vec::new();
    for (row, sender) in &candidates {
        if batchable.contains(&row.id) {
            if Some(row.id) == lead_id {
                rows.push(row.clone());
            } else if lead_id.is_some() {
                passengers.push(row.clone());
            }
        } else if wants_wake(row) && !capped[sender] {
            facts.entry(row.id).or_default().message_senders = vec![sender.clone()];
            rows.push(row.clone());
        }
    }
    if let Some(lead_id) = lead_id {
        let fact = facts.get_mut(&lead_id).unwrap();
        fact.message_batch = passengers;
        fact.message_senders = batch_wakers
            .into_iter()
            .map(|(_, sender)| sender.clone())
            .collect();
        fact.message_senders.sort();
        fact.message_senders.dedup();
    }

    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Usage {
    input: u64,
    output: u64,
    started: Option<DateTime<Utc>>,
    complete: bool,
}

fn machine_turn_names(text: &str, id: i64) -> bool {
    let mut text = text.trim_start().trim_start_matches("\x1b[200~");
    if text.starts_with("<pasted_content ") {
        let Some((_, body)) = text.split_once('>') else {
            return false;
        };
        text = body;
    }
    if text.starts_with("Message from ") {
        let Some((_, body)) = text.split_once(": ") else {
            return false;
        };
        text = body;
    }
    text.starts_with(&format!("[cas #{id} "))
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
                    .find(|id| texts.iter().any(|text| machine_turn_names(text, **id)))
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
        if let Some(agent) = agents
            .iter()
            .filter(|agent| agent.name == recipient)
            .max_by_key(|agent| agent.registered_at)
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
                if usage.complete && usage.started.is_some() {
                    conn.execute("UPDATE supervisor_message_wakes SET started_at=?1,input_tokens=?2,output_tokens=?3,usage_evidence=?4 WHERE lead_id=?5 AND input_tokens IS NULL",
                        params![usage.started.map(|at|at.to_rfc3339()), i64::try_from(usage.input)?,i64::try_from(usage.output)?,path.display().to_string(),id])?;
                }
            }
        }
    }
    let conn = db
        .lock()
        .map_err(|_| anyhow::anyhow!("wake ledger lock poisoned"))?;
    let since = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let wakes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM supervisor_message_wakes WHERE recipient=?1 AND injected_at>?2",
        params![recipient, since],
        |row| row.get(0),
    )?;
    let mut query=conn.prepare("SELECT input_tokens+output_tokens FROM supervisor_message_wakes WHERE recipient=?1 AND injected_at>?2 AND input_tokens IS NOT NULL ORDER BY input_tokens+output_tokens")?;
    let totals: Vec<i64> = query
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
        "directed_message_wakes_last_hour: {wakes}\nmedian_tokens_per_woken_turn: {median}; measured_turns: {}\nmax_observed_start_latency_ms: {}\n",
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
    fn cas_bc4d_batching_errors_leave_action_selection_intact() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        for failure in ["config", "ledger", "park"] {
            let (_root, cas, peer) = setup();
            let ordinary = enqueue(&cas, &peer, "A peer field report");
            let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
            let lifecycle = queue.enqueue_urgent_with_outcome(
                "lifecycle-wake:test", "supervisor", "<ci-red-run>Review CI</ci-red-run>",
                Some("wake-session"), None, None, false, Some(&QueueOrigin::Daemon),
            ).unwrap().id();
            let blocker = enqueue(&cas, &peer, &crate::prompt_revalidation::attach_blocker_envelope(
                "A decision is required", &peer.name, None,
            ));
            let merge = enqueue(&cas, &peer, &crate::prompt_revalidation::attach_merge_request_envelope(
                "Please merge the delivery", &crate::prompt_revalidation::MergeRequestEnvelope {
                    task_id: "cas-bc4d".into(), branch_tip: "delivery".into(),
                    target_branch: "epic/test".into(), target_branch_tip: "base".into(),
                    anchor_tip: None, commits_not_on_target_base: 1, pr_number: None,
                    source_freshness_unavailable: None,
                },
            ));
            match failure {
                "config" => std::fs::write(cas.join("config.toml"), "[factory\n").unwrap(),
                "ledger" => {
                    db(&cas).unwrap().lock().unwrap().execute_batch(
                        "DROP TABLE supervisor_message_wake_pairs"
                    ).unwrap();
                }
                "park" => {
                    rusqlite::Connection::open(cas.join("cas.db")).unwrap().execute_batch(&format!(
                        "CREATE TRIGGER fail_wake_park BEFORE UPDATE OF next_attempt_at ON prompt_queue
                         WHEN NEW.id={ordinary} AND NEW.next_attempt_at IS NOT NULL
                         BEGIN SELECT RAISE(FAIL,'wake park unavailable'); END;"
                    )).unwrap();
                }
                _ => unreachable!(),
            }
            let logs = Arc::new(Mutex::new(Vec::new()));
            let sink = logs.clone();
            let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false)
                .with_max_level(tracing::Level::WARN)
                .with_writer(move || Capture(sink.clone())).finish();
            let selected = tracing::subscriber::with_default(subscriber, || {
                let selected = select(&cas);
                select(&cas);
                select(&cas);
                let count = || String::from_utf8(logs.lock().unwrap().clone()).unwrap()
                    .matches("supervisor_message_batching_failed").count();
                assert_eq!(count(), 1, "{failure}: persistent failure logged every pass");
                let later = Instant::now() + FAILURE_LOG_INTERVAL;
                warn_batching_failure(&cas, &anyhow::anyhow!("still unavailable"), later);
                warn_batching_failure(&cas, &anyhow::anyhow!("still unavailable"), later + Duration::from_secs(1));
                selected
            });
            for id in [ordinary, lifecycle, blocker, merge] {
                assert!(selected.rows.iter().any(|row| row.id == id),
                    "{failure}: batching failure must not lose selected row #{id}");
                assert!(selected.facts[&id].message_batch.is_empty());
            }
            let log = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
            // pin: this incident stage identifies the optional batching failure.
            assert!(log.contains("WARN") && log.contains("supervisor_message_batching_failed"),
                "{failure}: unavailable batching must be visible: {log}");
            assert_eq!(log.matches("supervisor_message_batching_failed").count(), 2,
                "{failure}: log once initially and once after the interval, not on every pass: {log}");
        }
    }
    #[test]
    fn waiting_leaders_skip_bad_timestamps_without_reloading_rows_cas_626af() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        for corrupt in ["unused-source", "deadline"] {
            let (_root, cas, peer) = setup();
            let leader = enqueue(&cas, &peer, "Earlier report");
            let bad = enqueue(&cas, &peer, "Malformed parked row");
            let fresh = enqueue(&cas, &peer, "New report");
            let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
            let row = queue.queued_prompt(fresh).unwrap().unwrap();
            let now = row.created_at;
            let deadline = now + chrono::Duration::seconds(5);
            queue.park_for_busy_recipient(leader, Some(DEBOUNCE_DETAIL), deadline).unwrap();
            queue.park_for_busy_recipient(bad, Some(DEBOUNCE_DETAIL), now + chrono::Duration::seconds(8)).unwrap();
            let conn = Connection::open(cas.join("cas.db")).unwrap();
            if corrupt == "deadline" {
                conn.execute("UPDATE prompt_queue SET next_attempt_at='zz-invalid' WHERE id=?1", [bad]).unwrap();
            } else {
                // Deadline inheritance authenticates the origin, not the
                // display source. A full row reload needlessly decodes it.
                conn.execute("UPDATE prompt_queue SET source=x'FF' WHERE id=?1", [bad]).unwrap();
            }
            let mut rows = vec![row];
            let mut facts = HashMap::from([(fresh, RowFacts::default())]);
            schedule(&cas, "wake-session", "wake-sup", &[peer], &mut rows, &mut facts, now)
                .expect("malformed parked metadata must not disable healthy batching");
            assert!(rows.is_empty(), "the new report should still debounce");
            let actual: String = conn.query_row("SELECT next_attempt_at FROM prompt_queue WHERE id=?1", [fresh], |row| row.get(0)).unwrap();
            assert_eq!(DateTime::parse_from_rfc3339(&actual).unwrap().with_timezone(&Utc), deadline,
                "{corrupt}: the healthy leader's earlier deadline is retained");
        }
    }
    #[test]
    fn cas_1e7e2_acknowledgements_wait_readable_for_the_next_turn() {
        let (_root, cas, peer) = setup();
        let ack = enqueue(&cas, &peer, "thanks, received");
        let receipt = enqueue(
            &cas,
            &peer,
            "[CAS reply: explicitly acknowledges notification_id=1]\n<cas-message-quiet/>\nAcknowledged.",
        );
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
                    // Claude inserts a newline inside its pasted-content wrapper (cas-7176).
                    {"type":"user","timestamp":"2026-10-20T17:00:00Z","message":{"content":"\n\n<pasted_content id=\"x\">\n[cas #42 peer] field report\n</pasted_content>"}},
                    {"type":"assistant","message":{"id":"a","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":5}}},
                    {"type":"user","message":{"content":[{"type":"tool_result","content":"tool response"}]}},
                    {"type":"assistant","message":{"id":"a","stop_reason":"end_turn","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":9}}},
                    {"type":"user","message":{"content":"Operator quoting [cas #99 peer] unrelated next turn"}},
                    {"type":"assistant","message":{"id":"b","stop_reason":"end_turn","usage":{"input_tokens":999,"output_tokens":999}}}
                ]),
                (100, 9),
            ),
            (
                "codex",
                serde_json::json!([
                    {"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"output_tokens":100}}}},
                    {"type":"response_item","timestamp":"2026-10-20T17:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Message from peer: \n[cas #42 peer] field report"}]}},
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
            let usage = transcript_usage(&path, &HashSet::from([42, 99])).unwrap();
            assert!(
                !usage.contains_key(&99),
                "quoted ids never attribute wake cost"
            );
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
