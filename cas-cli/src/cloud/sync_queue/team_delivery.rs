//! Delivery authority for retiring redundant personal transport.
//!
//! `team_delivery_ack:<personal-id>` in sync_metadata is a versioned receipt,
//! not policy evidence. Only an itemized inserted/updated response may create
//! it. Readers are this module's preview and transactional cleanup; generic
//! metadata readers see identifiers and a digest, never the private payload.
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{EntityType, QueuedSync, SyncOperation, SyncQueue};
use crate::error::CasError;

const ACK_PREFIX: &str = "team_delivery_ack:";

#[derive(Serialize, Deserialize)]
struct TeamDeliveryAck {
    version: u8,
    queue_id: i64,
    created_at: DateTime<Utc>,
    entity_type: EntityType,
    entity_id: String,
    operation: SyncOperation,
    team_id: String,
    project_id: String,
    payload_sha256: String,
}

/// Safe to print: contains no queued payload or private diagnostic text.
#[derive(Debug, Serialize)]
pub struct TeamOwnedCleanupRow {
    pub id: i64,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub eligible: bool,
    pub reason: &'static str,
}

fn payload_digest(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    Some(hex::encode(Sha256::digest(
        serde_json::to_vec(&value).ok()?,
    )))
}

fn personal_rows(conn: &Connection) -> Result<Vec<QueuedSync>, CasError> {
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, entity_id, operation, payload, team_id, project_id,
         created_at, retry_count, last_error, last_outcome, last_reason, failed_client_version
         FROM sync_queue WHERE team_id = '' ORDER BY id",
    )?;
    Ok(stmt
        .query_map([], SyncQueue::map_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn receipt_matches(
    conn: &Connection,
    item: &QueuedSync,
    team_id: &str,
    project_id: &str,
) -> Result<bool, CasError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_metadata WHERE key = ?1",
            [format!("{ACK_PREFIX}{}", item.id)],
            |row| row.get(0),
        )
        .optional()?;
    let Some(ack) = raw.and_then(|raw| serde_json::from_str::<TeamDeliveryAck>(&raw).ok()) else {
        return Ok(false);
    };
    Ok(ack.version == 1
        && !team_id.is_empty()
        && !project_id.is_empty()
        && ack.queue_id == item.id
        && ack.created_at == item.created_at
        && ack.entity_type == item.entity_type
        && ack.entity_id == item.entity_id
        && ack.operation == item.operation
        && item.operation == SyncOperation::Upsert
        && ack.team_id == team_id
        && ack.project_id == project_id
        && item.project_id.is_none()
        && item.payload.as_deref().and_then(payload_digest).as_deref() == Some(&ack.payload_sha256))
}

fn remove_with_receipt(conn: &Connection, id: i64) -> Result<(), CasError> {
    conn.execute(
        "DELETE FROM sync_queue WHERE id = ?1 AND team_id = ''",
        [id],
    )?;
    conn.execute(
        "DELETE FROM sync_metadata WHERE key = ?1",
        [format!("{ACK_PREFIX}{id}")],
    )?;
    Ok(())
}

impl SyncQueue {
    pub(crate) fn personal_delivery_snapshots(&self) -> Result<Vec<QueuedSync>, CasError> {
        personal_rows(&self.conn.lock().unwrap())
    }

    /// Persist equivalent-delivery proof before consuming an acknowledged team
    /// snapshot. Caller must have an itemized inserted/updated response for the
    /// exact transmitted value. LWW and aggregate acknowledgements never call
    /// this method. Any transformation of the queued JSON fails closed.
    pub(crate) fn acknowledge_team_write(
        &self,
        item: &QueuedSync,
        transmitted: &serde_json::Value,
        team_id: &str,
        project_id: &str,
        personal_snapshots: &[QueuedSync],
    ) -> Result<(), CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let transmitted_digest = hex::encode(Sha256::digest(
            serde_json::to_vec(transmitted).map_err(|e| CasError::Other(e.to_string()))?,
        ));
        let equivalent = item.operation == SyncOperation::Upsert
            && item.team_id.as_deref() == Some(team_id)
            && !team_id.is_empty()
            && !project_id.is_empty()
            && item
                .project_id
                .as_deref()
                .is_none_or(|project| project == project_id)
            && matches!(
                item.entity_type,
                EntityType::Entry
                    | EntityType::Task
                    | EntityType::Rule
                    | EntityType::Skill
                    | EntityType::TaskDependency
            )
            && transmitted.get("origin_project").and_then(|v| v.as_str()) == Some(project_id)
            && transmitted.get("scope").and_then(|v| v.as_str()) != Some("global")
            && transmitted.get("share").and_then(|v| v.as_str()) != Some("private")
            && item.payload.as_deref().and_then(payload_digest).as_deref()
                == Some(&transmitted_digest);
        if equivalent {
            for personal in personal_snapshots {
                if personal.entity_type != item.entity_type
                    || personal.entity_id != item.entity_id
                    || personal.operation != item.operation
                    || personal.project_id.is_some()
                    || personal
                        .payload
                        .as_deref()
                        .and_then(payload_digest)
                        .as_deref()
                        != Some(&transmitted_digest)
                {
                    continue;
                }
                let unchanged: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_queue
                    WHERE id = ?1 AND team_id = '' AND entity_type = ?2 AND entity_id = ?3
                    AND operation = ?4 AND payload IS ?5 AND created_at = ?6 AND project_id IS NULL)",
                    params![personal.id, personal.entity_type.as_str(), personal.entity_id,
                        personal.operation.as_str(), personal.payload, personal.created_at.to_rfc3339()],
                    |row| row.get(0))?;
                if !unchanged {
                    continue;
                }
                let ack = TeamDeliveryAck {
                    version: 1,
                    queue_id: personal.id,
                    created_at: personal.created_at,
                    entity_type: personal.entity_type,
                    entity_id: personal.entity_id.clone(),
                    operation: personal.operation,
                    team_id: team_id.to_owned(),
                    project_id: project_id.to_owned(),
                    payload_sha256: transmitted_digest.clone(),
                };
                let raw =
                    serde_json::to_string(&ack).map_err(|e| CasError::Other(e.to_string()))?;
                tx.execute(
                    "INSERT INTO sync_metadata(key, value) VALUES (?1, ?2)
                    ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![format!("{ACK_PREFIX}{}", personal.id), raw],
                )?;
            }
        }
        // A local edit can replace the same queue ID while HTTP is in flight.
        // Only consume the exact snapshot we sent; leave any replacement queued.
        tx.execute(
            "DELETE FROM sync_queue WHERE id = ?1 AND team_id = ?2
            AND operation = ?3 AND payload IS ?4 AND created_at = ?5 AND project_id IS ?6",
            params![
                item.id,
                team_id,
                item.operation.as_str(),
                item.payload,
                item.created_at.to_rfc3339(),
                item.project_id
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Remove only personal snapshots with durable, matching team delivery.
    /// The row, routing authority and receipt are revalidated under one SQLite
    /// transaction. Legacy, changed, deleted and otherwise unproven rows stay.
    pub fn neutralize_team_only_personal(
        &self,
        team_id: &str,
        project_id: &str,
    ) -> Result<usize, CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut removed = 0;
        for item in personal_rows(&tx)? {
            if receipt_matches(&tx, &item, team_id, project_id)? {
                remove_with_receipt(&tx, item.id)?;
                removed += 1;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Preview exact rejection IDs, or remove only the selected proven copies.
    /// An empty selection is allowed only for preview. Ownership rejection is
    /// policy evidence and never makes a row eligible for deletion.
    pub fn purge_team_owned_personal_rejections(
        &self,
        team_id: &str,
        project_id: &str,
        ids: &[i64],
        dry_run: bool,
    ) -> Result<Vec<TeamOwnedCleanupRow>, CasError> {
        if !dry_run && ids.is_empty() {
            return Err(CasError::Other(
                "cleanup requires explicit --ids; preview with --purge-team-owned --dry-run".into(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let items = personal_rows(&tx)?;
        if ids
            .iter()
            .any(|id| !items.iter().any(|item| item.id == *id))
        {
            return Err(CasError::Other(
                "selected ID is not a current personal queue row; preview again".into(),
            ));
        }
        let mut rows = Vec::new();
        for item in items {
            let rejected = item.last_outcome.as_deref() == Some("rejected")
                && item.last_reason.as_deref() == Some("team_owned_project");
            if (!ids.is_empty() && !ids.contains(&item.id)) || (ids.is_empty() && !rejected) {
                continue;
            }
            let eligible = rejected && receipt_matches(&tx, &item, team_id, project_id)?;
            rows.push(TeamOwnedCleanupRow {
                id: item.id,
                entity_type: item.entity_type,
                entity_id: item.entity_id,
                eligible,
                reason: if eligible {
                    "equivalent_team_write_acknowledged"
                } else if !rejected {
                    "not_team_owned_personal_rejection"
                } else {
                    "no_matching_team_delivery_proof"
                },
            });
            if eligible && !dry_run {
                remove_with_receipt(&tx, item.id)?;
            }
        }
        tx.commit()?;
        Ok(rows)
    }
}
