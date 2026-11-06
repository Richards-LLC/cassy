//! The local logical-send ledger. Payload equality never implies send identity.
use super::*;
use rusqlite::{Connection, OptionalExtension, params};

pub const OUTBOX_TTL_SECS: i64 = 15 * 60;
const SEND_LEASE_SECS: i64 = 120;
const BUILD_ROUTE_SEND_ATTEMPTS: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum PeerSendError {
    #[error("build route peer {0} is in its 90-second routing cooldown")]
    BuildRouteCooldown(String),
    #[error("peer supervisor {0} has no receiving factory daemon")]
    NoReceivingDaemon(String),
    #[error("{cause}{retry}")]
    Failed { cause: String, retry: String },
}

#[derive(Debug, thiserror::Error)]
#[error("build route peer {0} is in its routing cooldown")]
struct BuildRouteCooldown(String);

pub(crate) fn build_route_recent_peers(root: &Path, since: i64) -> Result<std::collections::HashSet<String>, String> {
    let read = || -> anyhow::Result<_> {
        let conn = connection(root)?;
        let mut query = conn.prepare("SELECT DISTINCT json_extract(payload,'$.recipient_agent_id') FROM peer_message_outbox WHERE created_at>?1 AND json_valid(payload) AND json_extract(payload,'$.dedupe_key') LIKE 'build-route:%'")?;
        Ok(query.query_map([since], |row| row.get::<_, String>(0))?.collect::<Result<std::collections::HashSet<_>, _>>()?)
    };
    read().map_err(|error| error.to_string())
}

pub(crate) fn cancel_build_route(root: &Path, sender: &str, key: &str) -> Result<(), String> {
    let cancel = || -> anyhow::Result<()> {
        connection(root)?.execute("UPDATE peer_message_outbox SET state='abandoned',last_error='build route expired; local admission restored' WHERE sender_agent_id=?1 AND json_valid(payload) AND json_extract(payload,'$.dedupe_key')=?2 AND cloud_id IS NULL AND state IN ('pending','retrying')", params![sender,key])?;
        Ok(())
    };
    cancel().map_err(|error| error.to_string())
}

impl From<PeerSendError> for String {
    fn from(error: PeerSendError) -> Self {
        error.to_string()
    }
}

fn failed(cause: impl ToString, id: Option<i64>) -> PeerSendError {
    PeerSendError::Failed {
        cause: cause.to_string(),
        retry: id
            .map(|id| format!("; queued for automatic retry as peer-outbox:{id}"))
            .unwrap_or_default(),
    }
}

fn connection(root: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        root.join("cas.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    cas_store::shared_db::install_busy_handler(&conn)?;
    Ok(conn)
}

pub(super) fn enqueue_and_send(
    root: &Path,
    message: PeerSend,
    recipient: &str,
) -> Result<SendReceipt, PeerSendError> {
    enqueue_with_recovery(root, message, recipient, false)
}

/// Publication may re-attempt an abandoned receipt after the peer recovers.
/// Keep the original logical send key; ordinary messages retain their policy.
pub(super) fn enqueue_premerge_receipt(root: &Path, message: PeerSend, recipient: &str) -> Result<SendReceipt, PeerSendError> {
    enqueue_with_recovery(root, message, recipient, true)
}

fn enqueue_with_recovery(
    root: &Path, mut message: PeerSend, recipient: &str, recover_abandoned: bool,
) -> Result<SendReceipt, PeerSendError> {
    // Refuse before persisting: no login means no retry promise.
    http_mailbox(root).ok_or_else(|| failed("not logged in to Cassy Cloud", None))?;
    let sender = crate::store::open_agent_store(root)
        .and_then(|agents| {
            agents
                .get(&message.sender_agent_id)
                .map_err(crate::CasError::from)
        })
        .map_err(|e| failed(e, None))?;
    let result = (|| -> anyhow::Result<(i64, Option<String>)> {
        let mut conn = connection(root)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if !message.dedupe_key.is_empty() {
            let existing: Option<(i64, Option<String>, String)> = tx.query_row(
                "SELECT id,cloud_id,state FROM peer_message_outbox
                 WHERE sender_agent_id=?1
                 AND json_extract(CASE WHEN json_valid(payload) THEN payload END,'$.dedupe_key')=?2
                 AND json_extract(CASE WHEN json_valid(payload) THEN payload END,'$.recipient_agent_id')=?3 LIMIT 1",
                params![message.sender_agent_id, message.dedupe_key, message.recipient_agent_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?;
            if let Some((id, cloud_id, state)) = existing {
                if recover_abandoned && state == "abandoned" && cloud_id.is_none() {
                    let now = chrono::Utc::now().timestamp();
                    tx.execute("UPDATE peer_message_outbox SET state='pending',created_at=?2,next_attempt_at=?2,last_error=NULL WHERE id=?1 AND state='abandoned'", params![id,now])?;
                    tx.commit()?;
                }
                return Ok((id, cloud_id));
            }
        }
        let now = chrono::Utc::now().timestamp();
        if message.dedupe_key.starts_with("build-route:") {
            // The same immediate transaction selects the slot and persists
            // the ask: concurrent spawns cannot consume one peer twice.
            let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM peer_message_outbox WHERE created_at>?1 AND json_valid(payload) AND json_extract(payload,'$.dedupe_key') LIKE 'build-route:%' AND json_extract(payload,'$.recipient_agent_id')=?2)", params![now - 90, message.recipient_agent_id], |row| row.get(0))?;
            if occupied { return Err(BuildRouteCooldown(recipient.into()).into()); }
        }
        tx.execute("INSERT INTO peer_message_outbox(sender_agent_id,sender_name,sender_session,recipient,payload,created_at,next_attempt_at) VALUES (?1,?2,?3,?4,'',?5,?5)",
            params![sender.id,sender.name,sender.factory_session,recipient,now])?;
        let id = tx.last_insert_rowid();
        // A project-local row id, plus both registry identities: stable across
        // retries and daemon restarts, distinct for intentional repeated text.
        use sha2::{Digest, Sha256};
        let identity =
            serde_json::to_vec(&(&message.sender_agent_id, &message.recipient_agent_id, id))?;
        if message.dedupe_key.is_empty() {
            message.dedupe_key = format!(
                "peer:{}:{:x}",
                message.sender_agent_id.chars().take(64).collect::<String>(),
                Sha256::digest(identity)
            );
        }
        tx.execute(
            "UPDATE peer_message_outbox SET payload=?1 WHERE id=?2",
            params![serde_json::to_string(&message)?, id],
        )?;
        tx.commit()?;
        Ok((id, None))
    })();
    let (id, sent) = result.map_err(|error| {
        if let Some(cooldown) = error.downcast_ref::<BuildRouteCooldown>() {
            PeerSendError::BuildRouteCooldown(cooldown.0.clone())
        } else { failed(error, None) }
    })?;
    if let Some(id) = sent {
        return Ok(SendReceipt { id, status: "duplicate".into() });
    }
    retry_peer_send(root, id)
}

/// Retry one persisted logical send; every HTTP attempt uses its original key.
/// Normal callers rely on the daemon's automatic retry, rather than this API.
pub fn retry_peer_send(root: &Path, id: i64) -> Result<SendReceipt, PeerSendError> {
    match attempt(root, id, chrono::Utc::now().timestamp()).map_err(|e| failed(e, Some(id)))? {
        Some(result) => result,
        None => Err(failed(
            format!("peer-outbox:{id} is not due or is terminal; inspect message_status"),
            None,
        )),
    }
}

fn attempt(
    root: &Path,
    id: i64,
    now: i64,
) -> anyhow::Result<Option<Result<SendReceipt, PeerSendError>>> {
    let conn = connection(root)?;
    let pending: Option<String> = conn.query_row(
        "SELECT payload FROM peer_message_outbox WHERE id=?1 AND state IN ('pending','retrying')",
        [id], |row| row.get(0),
    ).optional()?;
    if let Some(payload) = pending {
        let message: PeerSend = serde_json::from_str(&payload)?;
        if reconcile_build_route(root, &message, false)? { return Ok(None); }
    }
    // Atomic lease prevents simultaneous MCP and daemon retries from owning a
    // row. The cloud key also protects against a crash after cloud acceptance.
    let claimed: Option<(String, u32)> = conn
        .query_row(
            "UPDATE peer_message_outbox SET state='retrying',attempts=attempts+1,next_attempt_at=?2
         WHERE id=?1 AND state IN ('pending','retrying') AND next_attempt_at<=?3 AND created_at>?4
         RETURNING payload,attempts",
            params![id, now + SEND_LEASE_SECS, now, now - OUTBOX_TTL_SECS],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((payload, attempts)) = claimed else {
        return Ok(None);
    };
    let message: PeerSend = serde_json::from_str(&payload)?;
    let result = http_mailbox(root)
        .ok_or_else(|| "not logged in to Cassy Cloud".to_string())
        .and_then(|m| m.send(&message));
    match result {
        Ok(receipt) => {
            conn.execute("UPDATE peer_message_outbox SET state='sent',cloud_id=?2,last_error=NULL WHERE id=?1 AND state='retrying' AND attempts=?3", params![id,receipt.id,attempts])?;
            Ok(Some(Ok(receipt)))
        }
        Err(cause) => {
            let retryable = retryable(&cause)
                && !(message.dedupe_key.starts_with("build-route:") && attempts >= BUILD_ROUTE_SEND_ATTEMPTS);
            let state = if retryable { "retrying" } else { "abandoned" };
            let delay = (5_i64 * (1_i64 << attempts.saturating_sub(1).min(3))).min(30);
            conn.execute("UPDATE peer_message_outbox SET state=?2,last_error=?3,next_attempt_at=?4 WHERE id=?1 AND state='retrying' AND attempts=?5", params![id,state,cause,now+delay,attempts])?;
            if !retryable { reconcile_build_route(root, &message, true)?; }
            Ok(Some(Err(failed(cause, retryable.then_some(id)))))
        }
    }
}

fn reconcile_build_route(root: &Path, message: &PeerSend, send_failed: bool) -> anyhow::Result<bool> {
    if !message.dedupe_key.starts_with("build-route:") { return Ok(false); }
    crate::factory_build_routing::reconcile_retry(
        root, &message.sender_agent_id, &message.dedupe_key, send_failed,
    ).map_err(anyhow::Error::msg)
}

fn retryable(cause: &str) -> bool {
    if cause.contains("not logged in") {
        return false;
    }
    // All permanent HTTP refusals stop. Throttling and request timeouts retry.
    !(400..500).any(|code| code != 408 && code != 429 && cause.contains(&format!("({code})")))
}

/// Local state, including the cloud id after acceptance. Does no network I/O.
pub fn outbox_status_text(root: &Path, id: &str) -> Result<Option<String>, String> {
    let Some(local) = id.strip_prefix("peer-outbox:") else {
        return Ok(None);
    };
    let local: i64 = local
        .parse()
        .map_err(|_| "invalid peer outbox id".to_string())?;
    (|| -> anyhow::Result<String> {
        let conn = connection(root)?;
        let (state, attempts, cloud, cause): (String, u32, Option<String>, Option<String>) = conn
            .query_row(
            "SELECT state,attempts,cloud_id,last_error FROM peer_message_outbox WHERE id=?1",
            [local],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        Ok(format!(
            "Peer outbox {id}: {state} (attempts: {attempts}){}{}",
            cloud
                .map(|id| format!(
                    "; cloud message {id}; check message_status id={id} for delivery"
                ))
                .unwrap_or_default(),
            cause
                .map(|cause| format!("; cause: {cause}"))
                .unwrap_or_default()
        ))
    })()
    .map(Some)
    .map_err(|e| e.to_string())
}

pub(super) fn tick(root: &Path, session: &str) -> Result<(), String> {
    tick_at(root, session, chrono::Utc::now().timestamp()).map_err(|e| e.to_string())
}

fn tick_at(root: &Path, _session: &str, now: i64) -> anyhow::Result<()> {
    let conn = connection(root)?;
    conn.execute("UPDATE peer_message_outbox SET state='abandoned',last_error=COALESCE(last_error,'mailbox unavailable') || '; retry TTL expired' WHERE state IN ('pending','retrying') AND created_at<=?1 AND next_attempt_at<=?2", params![now-OUTBOX_TTL_SECS,now])?;
    // Cancellation must not wait for next_attempt_at or another spawn. Also
    // recover an expiry interrupted between the terminal outbox write and note.
    let routes = {
        let mut stmt = conn.prepare("SELECT id,payload,state='abandoned' AND attempts>0 AND COALESCE(last_error,'')!='build route expired; local admission restored' FROM peer_message_outbox WHERE cloud_id IS NULL AND (state IN ('pending','retrying') OR (state='abandoned' AND notice_sent=0)) AND json_valid(payload) AND json_extract(payload,'$.dedupe_key') LIKE 'build-route:%'")?;
        stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, bool>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut failed_routes = std::collections::HashSet::new();
    for (id, payload, send_failed) in routes {
        if let Err(error) = serde_json::from_str::<PeerSend>(&payload)
            .map_err(anyhow::Error::from)
            .and_then(|message| reconcile_build_route(root, &message, send_failed))
        {
            tracing::warn!(outbox_id = id, %error, "build-route revalidation failed; leaving ask pending");
            failed_routes.insert(id);
        }
    }
    let notices = {
        let mut stmt = conn.prepare("SELECT id,sender_agent_id,sender_name,sender_session,recipient,last_error FROM peer_message_outbox WHERE state='abandoned' AND notice_sent=0")?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if !notices.is_empty() {
        let queue = crate::store::open_prompt_queue_store(root)?;
        for (id, sender, name, session, recipient, cause) in notices {
            notify_abandoned(
                queue.as_ref(),
                &sender,
                &name,
                session.as_deref(),
                id,
                &recipient,
                &cause,
            )?;
            conn.execute(
                "UPDATE peer_message_outbox SET notice_sent=1 WHERE id=?1",
                [id],
            )?;
        }
    }
    let due = {
        let mut stmt = conn.prepare("SELECT id FROM peer_message_outbox WHERE state IN ('pending','retrying') AND next_attempt_at<=?1 ORDER BY id")?;
        stmt.query_map(params![now], |r| r.get::<_, i64>(0))?
            .filter(|row| match row { Ok(id) => !failed_routes.contains(id), Err(_) => true })
            .take(2)
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in due {
        let _ = attempt(root, id, now)?;
    }
    Ok(())
}

fn notify_abandoned(
    queue: &dyn PromptQueueStore,
    sender: &str,
    name: &str,
    session: Option<&str>,
    id: i64,
    recipient: &str,
    cause: &str,
) -> anyhow::Result<()> {
    queue.notify_peer_send_abandoned(sender, name, session, id, recipient, cause)?;
    Ok(())
}

/// One ask this supervisor sent a peer that no peer reply answers yet
/// (cas-bddf9).
#[derive(Debug, Clone)]
pub struct OpenPeerAsk {
    /// Cloud peer message id; absent while the ask is still queued.
    pub cloud_id: Option<String>,
    pub recipient: String,
    pub body: String,
    /// An arrived answer that could not safely be associated with this ask.
    pub unlinked_answer: Option<String>,
}

struct PendingAsk {
    ask: OpenPeerAsk,
    sender_name: String,
    peer_id: Option<String>,
    created_at: i64,
    answered: bool,
}

impl PendingAsk {
    fn from_peer(&self, reply: &PeerEnvelope) -> bool {
        match self.peer_id.as_deref() {
            Some(id) => id == reply.sender_agent_id,
            None => self.ask.recipient.eq_ignore_ascii_case(
                &format!("{}@{}", reply.sender_name, reply.machine),
            ),
        }
    }
}

fn names_ask(body: &str, id: &str) -> bool {
    body.match_indices(id).any(|(at, _)| {
        let part = |c: char| c.is_ascii_alphanumeric() || c == '-';
        !body[..at].chars().next_back().is_some_and(part)
            && !body[at + id.len()..].chars().next().is_some_and(part)
    })
}

/// Open asks from `sender_agent_id` within `window_secs` (`kind=ask` or `blocker=true`).
/// Reply IDs survive succession. Unlinked typed answers need an open ask ID or
/// exactly one open ask per peer. Replay by arrival; exclude abandoned sends.
pub fn open_asks(root: &Path, sender_agent_id: &str, window_secs: i64) -> Vec<OpenPeerAsk> {
    let Ok(conn) = connection(root) else {
        return Vec::new();
    };
    let since = chrono::Utc::now().timestamp() - window_secs;
    let Ok(mut statement) = conn.prepare(
        "SELECT o.cloud_id, o.recipient, json_extract(o.payload, '$.body'),
                o.sender_name, json_extract(o.payload, '$.recipient_agent_id'), o.created_at
         FROM peer_message_outbox o
         WHERE o.sender_agent_id = ?1 AND o.created_at >= ?2 AND o.state != 'abandoned'
           AND (json_extract(o.payload, '$.body') LIKE '<cas-peer-intent kind=\"ask\"/>%'
                OR json_extract(o.payload, '$.body') LIKE '<cas-peer-intent kind=\"blocker\"/>%')
         ORDER BY o.id",
    ) else {
        return Vec::new();
    };
    let mut asks: Vec<_> = statement
        .query_map(params![sender_agent_id, since], |row| {
            Ok(PendingAsk {
                ask: OpenPeerAsk {
                    cloud_id: row.get(0)?,
                    recipient: row.get(1)?,
                    body: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    unlinked_answer: None,
                },
                sender_name: row.get(3)?,
                peer_id: row.get(4)?,
                created_at: row.get(5)?,
                answered: false,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    if asks.is_empty() {
        return Vec::new();
    }
    let replies = (|| -> rusqlite::Result<Vec<(String, String, String, i64)>> {
        let mut statement = conn.prepare(
            "SELECT source, target, prompt, CAST(strftime('%s', created_at) AS INTEGER) AS epoch
             FROM prompt_queue WHERE source LIKE 'peer:%' AND epoch >= ?1 ORDER BY epoch, id",
        )?;
        Ok(statement.query_map([since], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?.filter_map(Result::ok).collect())
    })().unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for (source, target, prompt, arrived_at) in replies {
        let Some(reply) = parse_envelope(&prompt) else { continue; };
        if source != format!("peer:{}@{}", reply.sender_name, reply.machine)
            || !seen.insert(reply.id.clone())
            || matches!(reply.kind, Some(PeerMessageKind::Receipt | PeerMessageKind::Quiet | PeerMessageKind::Status))
        {
            continue;
        }
        // Preserve the existing Cloud reply-link contract, including a live
        // successor answering mail routed from its retired predecessor.
        if let Some(id) = reply.in_reply_to.as_deref()
            && let Some(ask) = asks.iter_mut().find(|ask| ask.ask.cloud_id.as_deref() == Some(id))
        {
            ask.answered = true;
            continue;
        }
        let candidates: Vec<_> = asks.iter().enumerate().filter_map(|(i, ask)| {
            (!ask.answered && ask.created_at <= arrived_at && ask.from_peer(&reply)
                && (target == ask.sender_name || target == sender_agent_id)).then_some(i)
        }).collect();
        let answer = if reply.in_reply_to.is_none() && reply.kind == Some(PeerMessageKind::Answer) {
            let body = prompt.split_once('\n').map(|(_, body)| body).unwrap_or("");
            let named: Vec<_> = asks.iter().enumerate().filter_map(|(i, ask)| {
                ask.ask.cloud_id.as_deref().is_some_and(|id| names_ask(body, id)).then_some(i)
            }).collect();
            match named.as_slice() {
                [i] if candidates.contains(i) => Some(*i),
                [] if candidates.len() == 1 => Some(candidates[0]),
                _ => None,
            }
        } else {
            None
        };
        if let Some(i) = answer.filter(|&i| asks[i].ask.cloud_id.is_some()) {
            asks[i].answered = true;
        } else if reply.kind == Some(PeerMessageKind::Answer) {
            for i in candidates {
                asks[i].ask.unlinked_answer.get_or_insert_with(|| reply.id.clone());
            }
        }
    }
    asks.into_iter().filter(|ask| !ask.answered).map(|ask| ask.ask).collect()
}
