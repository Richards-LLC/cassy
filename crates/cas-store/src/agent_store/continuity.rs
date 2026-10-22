//! Retirement evidence is retained independently of disposable registry rows.
use super::SqliteAgentStore;
use crate::{Result, error::StoreError};
use cas_types::{Agent, AgentRole};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// One project/host-scoped lane snapshot. IDs and original ownership are evidence,
/// not an instruction to transfer live workers or overwrite current assignments.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionEnd {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub reason: String,
    pub registered_at: String,
    pub factory_session: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub ppid: Option<u32>,
    #[serde(default)]
    pub pid_starttime: Option<u64>,
    #[serde(default)]
    pub parent_starttime: Option<u64>,
    pub tasks: Vec<Value>,
    pub workers: Vec<Value>,
    pub pending: Vec<Value>,
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Only fixed internal SQL is accepted. Optional subsystems may never have been
/// initialized; their absence is empty evidence, while read failures abort capture.
fn rows(
    conn: &Connection,
    table: &str,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Value>> {
    if !table_exists(conn, table)? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    Ok(stmt
        .query_map(args, |row| {
            let mut object = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => Value::from(v),
                    ValueRef::Real(v) => Value::from(v),
                    ValueRef::Text(v) => Value::String(String::from_utf8_lossy(v).into_owned()),
                    ValueRef::Blob(_) => Value::Null,
                };
                object.insert(name.clone(), value);
            }
            Ok(Value::Object(object))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn capture(conn: &Connection, id: &str, reason: Option<&str>) -> Result<()> {
    let identity: Option<(String, String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT name, role, registered_at, machine_id, factory_session FROM agents WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((name, role, epoch, machine, session)) = identity else {
        return Ok(());
    };
    let reason = reason
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM agent_session_ends WHERE agent_id=? AND registered_at=?",
            params![id, epoch],
            |row| row.get(0),
        )
        .optional()?;
    if existing.is_some() {
        // Cleanup retries cannot replace the original snapshot or erase an
        // observed reason with a later reasonless retirement.
        if reason != "unknown" {
            conn.execute("UPDATE agent_session_ends SET reason=? WHERE agent_id=? AND registered_at=? AND reason='unknown'", params![reason, id, epoch])?;
        }
        return Ok(());
    }
    let (pid, ppid, pid_starttime, metadata): (Option<u32>, Option<u32>, Option<i64>, String) =
        conn.query_row(
            "SELECT pid,ppid,pid_starttime,metadata FROM agents WHERE id=?",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    let parent_starttime = serde_json::from_str::<Value>(&metadata)
        .ok()
        .and_then(|value| {
            value["ppid_starttime"]
                .as_str()
                .and_then(|s| s.parse().ok())
        });
    let mut handoff = SessionEnd {
        id: format!("{:x}", Sha256::digest(format!("{id}\0{epoch}").as_bytes())),
        agent_id: id.into(),
        name: name.clone(),
        reason: reason.into(),
        registered_at: epoch.clone(),
        factory_session: session.clone(),
        pid,
        ppid,
        pid_starttime: pid_starttime.and_then(|value| u64::try_from(value).ok()),
        parent_starttime,
        tasks: Vec::new(),
        workers: Vec::new(),
        pending: Vec::new(),
    };
    if role == "supervisor" {
        handoff.workers = rows(
            conn,
            "agents",
            "SELECT id,name,status,pid,factory_session FROM agents WHERE role='worker' AND (parent_id=?1 OR (factory_session=?2 AND ?2 IS NOT NULL)) ORDER BY id",
            &[&id, &session],
        )?;
        // Preserve unadopted lane evidence across a chain of successors. Use
        // current task/worker rows for those IDs, never replay old ownership.
        let mut inherited_tasks = std::collections::HashSet::new();
        let mut inherited_workers = std::collections::HashSet::new();
        let mut inherited = conn.prepare("SELECT id,payload FROM agent_session_ends WHERE claimed_by=? AND claimed_epoch=? AND queued_at IS NOT NULL")?;
        let inherited = inherited
            .query_map(params![id, epoch], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut inherited_receipts = Vec::new();
        for (receipt, payload) in inherited {
            let previous: SessionEnd =
                serde_json::from_str(&payload).map_err(|e| StoreError::Other(e.to_string()))?;
            for task in previous.tasks {
                if let Some(id) = task["id"].as_str() {
                    inherited_tasks.insert(id.to_string());
                }
            }
            for worker in previous.workers {
                if let Some(id) = worker["id"].as_str() {
                    inherited_workers.insert(id.to_string());
                }
            }
            inherited_receipts.push(
                serde_json::json!({"inherited_handoff":receipt,"predecessor":previous.agent_id}),
            );
        }
        let current_workers = rows(
            conn,
            "agents",
            "SELECT id,name,status,pid,factory_session FROM agents WHERE role='worker' ORDER BY id",
            &[],
        )?;
        for worker in current_workers {
            if worker["id"]
                .as_str()
                .is_some_and(|id| inherited_workers.contains(id))
                && !handoff.workers.iter().any(|w| w["id"] == worker["id"])
            {
                handoff.workers.push(worker);
            }
        }
        let mut owners = vec![id.to_string(), name.clone()];
        for worker in &handoff.workers {
            for key in ["id", "name"] {
                if let Some(owner) = worker[key].as_str() {
                    owners.push(owner.to_string());
                }
            }
        }
        let epic_ids = rows(
            conn,
            "working_epics",
            "SELECT epic_id FROM working_epics WHERE agent_id=?",
            &[&id],
        )?;
        handoff.tasks = rows(conn, "tasks", "SELECT id,title,status,assignee,epic_verification_owner,deliverables,external_ref,notes FROM tasks WHERE status NOT IN ('closed','cancelled') ORDER BY id", &[])?
            .into_iter().filter(|task| {
                ["assignee", "epic_verification_owner"].iter().any(|key| task[*key].as_str().is_some_and(|owner| owners.iter().any(|s| s == owner)))
                    || epic_ids.iter().any(|epic| epic["epic_id"] == task["id"])
                    || task["id"].as_str().is_some_and(|id| inherited_tasks.contains(id))
            }).collect();
        // Notes are retained in the receipt; put them last in startup projections
        // so a long timeline cannot conceal the delivery ref or PR.
        for task in &mut handoff.tasks {
            if let Some(encoded) = task["deliverables"].as_str()
                && let Ok(deliverables) = serde_json::from_str::<Value>(encoded)
            {
                task["deliverables"] = deliverables;
            }
        }
        handoff.pending = rows(
            conn,
            "prompt_queue",
            "SELECT id,source,target,prompt FROM prompt_queue WHERE processed_at IS NULL AND (target=?1 OR source=?1 OR target=?2 OR source=?2) ORDER BY id",
            &[&name, &id],
        )?;
        handoff.pending.extend(rows(conn, "supervisor_queue", "SELECT id,event_type,payload FROM supervisor_queue WHERE supervisor_id=? AND processed_at IS NULL ORDER BY id", &[&id])?);
        handoff.pending.extend(rows(conn, "reminders", "SELECT id,message,trigger_type,trigger_at,trigger_event,trigger_filter FROM reminders WHERE supervisor_id=? AND status IN ('pending','suspended') ORDER BY id", &[&id])?);
        // A delivered Commander ask is still open while its deadline reminder
        // is pending. Include its original body even after transport processed it.
        let mut asks = Vec::new();
        for reminder in &handoff.pending {
            if let Some(filter) = reminder["trigger_filter"].as_str()
                && let Ok(filter) = serde_json::from_str::<Value>(filter)
                && let Some(ask_id) = filter["operator_ask"].as_i64()
            {
                asks.extend(rows(
                    conn,
                    "prompt_queue",
                    "SELECT id,source,target,prompt FROM prompt_queue WHERE id=?",
                    &[&ask_id],
                )?);
            }
        }
        handoff.pending.extend(asks);
        handoff.pending.extend(inherited_receipts);
    }
    let encoded = serde_json::to_string(&handoff).map_err(|e| StoreError::Other(e.to_string()))?;
    conn.execute("INSERT INTO agent_session_ends (id,agent_id,registered_at,machine_id,role,reason,payload,ended_at) VALUES (?,?,?,?,?,?,?,?)", params![handoff.id,id,epoch,machine,role,reason,encoded,Utc::now().to_rfc3339()])?;
    Ok(())
}

pub(super) fn end_reason(conn: &Connection, id: &str) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT reason FROM agent_session_ends WHERE agent_id=? ORDER BY ended_at DESC LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "unknown".into()))
}

fn decode(payload: String, reason: String) -> Result<SessionEnd> {
    let mut handoff: SessionEnd =
        serde_json::from_str(&payload).map_err(|e| StoreError::Other(e.to_string()))?;
    handoff.reason = reason;
    Ok(handoff)
}

impl SqliteAgentStore {
    /// Record observed cause before any destructive retirement. Unknown stays
    /// unknown: heartbeat/PID absence alone cannot prove a crash or operator exit.
    pub fn record_session_end(&self, id: &str, reason: Option<&str>) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            capture(&tx, id, reason)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// One winner per predecessor, even with concurrent successor startups.
    /// Failed outbox publication remains retryable by that winner.
    pub fn claim_supervisor_handoffs(&self, successor: &Agent) -> Result<Vec<SessionEnd>> {
        if successor.role != AgentRole::Supervisor
            || !successor.is_alive()
            || successor.machine_id.is_none()
        {
            return Ok(Vec::new());
        }
        let conn = self.lock_conn()?;
        let tx = crate::shared_db::ImmediateTx::new(&conn)?;
        let epoch = successor.registered_at.to_rfc3339();
        let mut stmt = tx.prepare(
            "SELECT e.id,e.payload,e.reason FROM agent_session_ends e
             WHERE e.role='supervisor' AND e.machine_id=?1 AND e.registered_at<?2
               AND e.queued_at IS NULL
               AND EXISTS (SELECT 1 FROM agents s WHERE s.id=?3 AND s.registered_at=?2 AND s.machine_id=?1 AND s.role='supervisor' AND s.status IN ('active','idle'))
               AND (e.claimed_by IS NULL OR (e.claimed_by=?3 AND e.claimed_epoch=?2)
                    OR NOT EXISTS (SELECT 1 FROM agents c WHERE c.id=e.claimed_by AND c.registered_at=e.claimed_epoch AND c.status IN ('active','idle')))
               AND NOT EXISTS (SELECT 1 FROM agents a WHERE a.id=e.agent_id AND a.registered_at=e.registered_at AND a.status IN ('active','idle'))
             ORDER BY e.ended_at")?;
        let candidates = stmt
            .query_map(params![successor.machine_id, epoch, successor.id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut result = Vec::new();
        for (id, payload, reason) in candidates {
            tx.execute(
                "UPDATE agent_session_ends SET claimed_by=?,claimed_epoch=? WHERE id=?",
                params![successor.id, epoch, id],
            )?;
            result.push(decode(payload, reason)?);
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn mark_supervisor_handoff_queued(&self, id: &str, successor: &Agent) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute("UPDATE agent_session_ends SET queued_at=? WHERE id=? AND claimed_by=? AND claimed_epoch=?", params![Utc::now().to_rfc3339(),id,successor.id,successor.registered_at.to_rfc3339()])?;
        Ok(())
    }

    /// Retained worker identity is authority even after optional activity-event
    /// retention. Keep exact process fingerprints so retirement cannot hide a
    /// still-running harness from a recovery request.
    pub fn retired_workers_named(&self, identity: &str) -> Result<Vec<SessionEnd>> {
        let conn = self.lock_conn()?;
        let mut statement = conn.prepare("SELECT payload,reason FROM agent_session_ends WHERE role='worker' AND (agent_id=?1 OR json_extract(payload,'$.name')=?1) ORDER BY ended_at DESC")?;
        let records = statement
            .query_map([identity], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        records
            .into_iter()
            .map(|(payload, reason)| decode(payload, reason))
            .collect()
    }

    pub fn recent_session_ends(&self, machine: &str, limit: usize) -> Result<Vec<SessionEnd>> {
        let conn = self.lock_conn()?;
        if !table_exists(&conn, "agent_session_ends")? {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare("SELECT e.payload,e.reason FROM agent_session_ends e WHERE e.role='supervisor' AND e.machine_id=? ORDER BY e.ended_at DESC LIMIT ?")?;
        let rows = stmt
            .query_map(params![machine, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(payload, reason)| decode(payload, reason))
            .collect()
    }
}
