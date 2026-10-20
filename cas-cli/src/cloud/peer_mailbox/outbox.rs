//! The local logical-send ledger. Payload equality never implies send identity.
use super::*;
use rusqlite::{Connection, OptionalExtension, params};

pub const OUTBOX_TTL_SECS: i64 = 15 * 60;
const SEND_LEASE_SECS: i64 = 120;

#[derive(Debug, thiserror::Error)]
pub enum PeerSendError {
    #[error("peer supervisor {0} has no receiving factory daemon")]
    NoReceivingDaemon(String),
    #[error("{cause}{retry}")]
    Failed { cause: String, retry: String },
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
    mut message: PeerSend,
    recipient: &str,
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
            let existing: Option<(i64, Option<String>)> = tx.query_row(
                "SELECT id,cloud_id FROM peer_message_outbox
                 WHERE sender_agent_id=?1
                 AND json_extract(CASE WHEN json_valid(payload) THEN payload END,'$.dedupe_key')=?2
                 AND json_extract(CASE WHEN json_valid(payload) THEN payload END,'$.recipient_agent_id')=?3 LIMIT 1",
                params![message.sender_agent_id, message.dedupe_key, message.recipient_agent_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some(existing) = existing {
                return Ok(existing);
            }
        }
        let now = chrono::Utc::now().timestamp();
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
    let (id, sent) = result.map_err(|e| failed(e, None))?;
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
            let retryable = retryable(&cause);
            let state = if retryable { "retrying" } else { "abandoned" };
            let delay = (5_i64 * (1_i64 << attempts.saturating_sub(1).min(3))).min(30);
            conn.execute("UPDATE peer_message_outbox SET state=?2,last_error=?3,next_attempt_at=?4 WHERE id=?1 AND state='retrying' AND attempts=?5", params![id,state,cause,now+delay,attempts])?;
            Ok(Some(Err(failed(cause, retryable.then_some(id)))))
        }
    }
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
        let mut stmt = conn.prepare("SELECT id FROM peer_message_outbox WHERE state IN ('pending','retrying') AND next_attempt_at<=?1 ORDER BY id LIMIT 2")?;
        stmt.query_map(params![now], |r| r.get::<_, i64>(0))?
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
