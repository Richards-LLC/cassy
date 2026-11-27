use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

fn connection(root: &Path) -> Result<Connection> {
    let db = Connection::open(root.join("cas.db"))?;
    db.busy_timeout(std::time::Duration::from_millis(100))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS efficiency_observations (
        kind TEXT NOT NULL, observation_key TEXT NOT NULL, occurred_at TEXT NOT NULL,
        payload TEXT NOT NULL, PRIMARY KEY(kind, observation_key))",
    )?;
    Ok(db)
}

fn record(db: &Connection, kind: &str, key: &str, at: DateTime<Utc>, payload: Value) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO efficiency_observations VALUES (?1,?2,?3,?4)",
        params![kind, key, at.to_rfc3339(), payload.to_string()],
    )?;
    Ok(())
}

pub(crate) fn tool_succeeded(
    root: &Path,
    actor: &str,
    tool: &str,
    action: &str,
    task: Option<&str>,
    at: DateTime<Utc>,
) {
    let result = (|| -> Result<()> {
        let db = connection(root)?;
        let mut stmt = db.prepare("SELECT id FROM events WHERE event_type='gate_refused'
            AND json_valid(metadata) AND json_extract(metadata,'$.actor')=?1
            AND json_extract(metadata,'$.tool')=?2 AND json_extract(metadata,'$.action')=?3
            AND json_extract(metadata,'$.task_id') IS ?4 AND julianday(created_at)<=julianday(?5)
            AND NOT EXISTS (SELECT 1 FROM efficiency_observations o WHERE o.kind='refusal_recovered' AND o.observation_key=CAST(events.id AS TEXT))")?;
        let ids = stmt
            .query_map(params![actor, tool, action, task, at.to_rfc3339()], |r| {
                r.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            record(
                &db,
                "refusal_recovered",
                &id.to_string(),
                at,
                json!({"actor":actor,"tool":tool,"action":action,"task_id":task}),
            )?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(%error, "cannot record refusal recovery");
    }
}

pub(crate) fn peer_admitted(
    root: &Path,
    message: &crate::cloud::peer_mailbox::ClaimedPeerMessage,
    at: DateTime<Utc>,
) {
    let result = connection(root).and_then(|db| record(&db, "peer_admitted", &message.id, at,
        json!({"created_at":message.created_at,"machine":message.sender_machine(),"recipient":message.recipient_agent_id})));
    if let Err(error) = result {
        tracing::warn!(%error, "cannot record peer admission latency");
    }
}

pub(crate) fn issue_closed(root: &Path, issue: &str, at: DateTime<Utc>) {
    let result = connection(root).and_then(|db| {
        record(
            &db,
            "issue_closed",
            &format!("{issue}|{}", at.timestamp_millis()),
            at,
            json!({"issue":issue}),
        )
    });
    if let Err(error) = result {
        tracing::warn!(%error, "cannot record issue closure observation");
    }
}
