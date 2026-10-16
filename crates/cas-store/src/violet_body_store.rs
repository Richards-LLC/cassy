//! Managed original bodies need a remotely confirmed, one-use local permit.
//! A transport reservation or pointer wake is never such a permit.
use super::violet_admission::SqliteVioletAdmissionStore;
use super::*;
use sha2::{Digest, Sha256};

pub const VIOLET_BODY_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS violet_body_permits (
        prompt_id INTEGER PRIMARY KEY REFERENCES prompt_queue(id),
        delivery_id TEXT NOT NULL UNIQUE,
        registration_id TEXT NOT NULL,
        delivery_generation INTEGER NOT NULL,
        factory_session TEXT NOT NULL,
        recipient TEXT NOT NULL,
        instance_id TEXT,
        request_id TEXT,
        state TEXT NOT NULL DEFAULT 'blocked'
            CHECK(state IN ('blocked','armed','spent','revoked'))
    )",
    "CREATE TABLE IF NOT EXISTS violet_consumption_outbox (
        request_id TEXT PRIMARY KEY,
        delivery_id TEXT NOT NULL,
        registration_id TEXT NOT NULL,
        project_id TEXT NOT NULL,
        delivery_generation INTEGER NOT NULL,
        instance_id TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('dispatch_uncertain','consumed')),
        input_json TEXT NOT NULL,
        input_digest TEXT NOT NULL,
        response_json TEXT,
        retry_at INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0,
        UNIQUE(delivery_id,delivery_generation,state)
    )",
];

pub(super) const BODY_OUTBOX_IDENTITY_TRIGGER: &str = "
CREATE TRIGGER IF NOT EXISTS violet_consumption_identity_immutable
BEFORE UPDATE OF request_id,delivery_id,registration_id,project_id,delivery_generation,
    instance_id,state,input_json,input_digest ON violet_consumption_outbox
BEGIN SELECT RAISE(ABORT, 'Violet consumption request identity is immutable'); END;";

/// SQL uses the queue alias `q`. Also refuse an unbacked v2 envelope: copied
/// bytes cannot manufacture a managed permit.
pub(super) const BODY_ELIGIBLE_SQL: &str = r#"
AND (
    (NOT EXISTS(SELECT 1 FROM violet_body_permits vb WHERE vb.prompt_id=q.id)
        AND q.prompt NOT LIKE '<cas-violet-activity v="2"%')
    OR EXISTS(SELECT 1 FROM violet_body_permits vb WHERE vb.prompt_id=q.id
        AND vb.state='armed' AND vb.factory_session=q.factory_session
        AND vb.recipient=q.target)
)"#;

#[derive(Debug, Clone)]
pub struct VioletConsumptionJob {
    pub request_id: String,
    pub delivery_id: String,
    pub registration_id: String,
    pub project_id: String,
    pub delivery_generation: i64,
    pub instance_id: String,
    pub state: String,
    pub input_json: String,
    pub input_digest: String,
}

fn job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VioletConsumptionJob> {
    Ok(VioletConsumptionJob {
        request_id: row.get(0)?,
        delivery_id: row.get(1)?,
        registration_id: row.get(2)?,
        project_id: row.get(3)?,
        delivery_generation: row.get(4)?,
        instance_id: row.get(5)?,
        state: row.get(6)?,
        input_json: row.get(7)?,
        input_digest: row.get(8)?,
    })
}
const JOB_COLUMNS: &str = "request_id,delivery_id,registration_id,project_id,
    delivery_generation,instance_id,state,input_json,input_digest";

impl SqliteVioletAdmissionStore {
    pub fn body_dispatch_candidates(
        &self,
        registration: &str,
        project: &str,
        instance: &str,
    ) -> Result<Vec<String>> {
        let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
        let mut stmt = conn.prepare("SELECT a.delivery_id FROM violet_admissions a
            JOIN violet_body_permits b ON b.prompt_id=a.prompt_id
            WHERE a.registration_id=?1 AND a.project_id=?2 AND a.ack_pending=0 AND a.consumption='queued'
              AND b.state='blocked' AND (b.instance_id IS NULL OR b.instance_id=?3)
            ORDER BY a.rowid LIMIT 25")?;
        Ok(stmt
            .query_map(params![registration, project, instance], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    /// Freeze intent before HTTP. The same generation can never obtain a new
    /// original attempt after a crash; its existing UUID/body must reconcile.
    pub fn prepare_body_dispatch(
        &self,
        delivery: &str,
        registration: &str,
        project: &str,
        instance: &str,
        request_id: &str,
    ) -> Result<VioletConsumptionJob> {
        if instance.trim().is_empty() || request_id.trim().is_empty() {
            return Err(StoreError::Other(
                "body dispatch requires instance and request identity".into(),
            ));
        }
        crate::shared_db::with_immediate_write_txn_pooled(&self.queue.conn, |tx| {
            let generation: i64 = tx
                .query_row(
                    "SELECT a.delivery_generation FROM violet_admissions a
                 JOIN violet_body_permits b ON b.prompt_id=a.prompt_id
                 WHERE a.delivery_id=?1 AND a.registration_id=?2 AND a.project_id=?3
                   AND a.ack_pending=0 AND a.consumption='queued'
                   AND b.state IN ('blocked','armed')
                   AND b.registration_id=a.registration_id
                   AND b.delivery_generation=a.delivery_generation",
                    params![delivery, registration, project],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    StoreError::Other(
                        "admission is not eligible for an original body attempt".into(),
                    )
                })?;
            let existing = tx
                .query_row(
                    &format!(
                        "SELECT {JOB_COLUMNS} FROM violet_consumption_outbox
                WHERE delivery_id=?1 AND delivery_generation=?2 AND state='dispatch_uncertain'"
                    ),
                    params![delivery, generation],
                    job_from_row,
                )
                .optional()?;
            if let Some(job) = existing {
                if job.instance_id != instance || job.registration_id != registration {
                    return Err(StoreError::Other(
                        "prior body attempt requires recovery; never re-arm it".into(),
                    ));
                }
                return Ok(job);
            }
            let input_json = serde_json::json!({"schema":"cassy.coordination/v1",
                "request_id":request_id, "actor":{"registration_id":registration},
                "delivery_generation":generation, "state":"dispatch_uncertain",
                "observed_at":Utc::now().to_rfc3339()})
            .to_string();
            let input_digest = format!("{:x}", Sha256::digest(input_json.as_bytes()));
            tx.execute(
                "INSERT INTO violet_consumption_outbox
                (request_id,delivery_id,registration_id,project_id,delivery_generation,
                 instance_id,state,input_json,input_digest)
                VALUES (?1,?2,?3,?4,?5,?6,'dispatch_uncertain',?7,?8)",
                params![
                    request_id,
                    delivery,
                    registration,
                    project,
                    generation,
                    instance,
                    input_json,
                    input_digest
                ],
            )?;
            tx.execute(
                "UPDATE violet_body_permits SET instance_id=?2,request_id=?3
                WHERE delivery_id=?1 AND state='blocked'",
                params![delivery, instance, request_id],
            )?;
            Ok(VioletConsumptionJob {
                request_id: request_id.into(),
                delivery_id: delivery.into(),
                registration_id: registration.into(),
                project_id: project.into(),
                delivery_generation: generation,
                instance_id: instance.into(),
                state: "dispatch_uncertain".into(),
                input_json,
                input_digest,
            })
        })
    }

    /// Store a validated remote result and arm only the original live instance.
    /// Stale responses remain history and cannot revive a successor's body.
    pub fn confirm_body_dispatch(
        &self,
        job: &VioletConsumptionJob,
        current_instance: &str,
        response: &str,
    ) -> Result<bool> {
        let response_value: serde_json::Value =
            serde_json::from_str(response).map_err(|e| StoreError::Other(e.to_string()))?;
        let delivery = &response_value["receipt"]["delivery"];
        if response_value["schema"] != "cassy.coordination/v1"
            || response_value["code"] != "completed"
            || response_value["receipt"]["request_id"].as_str() != Some(job.request_id.as_str())
            || delivery["id"].as_str() != Some(job.delivery_id.as_str())
            || delivery["registration_id"].as_str() != Some(job.registration_id.as_str())
            || delivery["delivery_generation"].as_i64() != Some(job.delivery_generation)
            || delivery["consumption"].as_str() != Some(job.state.as_str())
            || delivery["status"] != "admitted"
        {
            return Err(StoreError::Other(
                "consumption receipt does not match the frozen request".into(),
            ));
        }
        crate::shared_db::with_immediate_write_txn_pooled(&self.queue.conn, |tx| {
            let identity_matches: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM violet_admissions
                WHERE delivery_id=?1 AND project_id=?2 AND occurrence_id=?3 AND reader_slot_id=?4)",
                params![
                    job.delivery_id,
                    job.project_id,
                    delivery["occurrence_id"].as_str(),
                    delivery["reader_slot_id"].as_str()
                ],
                |r| r.get(0),
            )?;
            if !identity_matches {
                return Err(StoreError::Other(
                    "consumption receipt has foreign occurrence/reader identity".into(),
                ));
            }
            let matching = tx.execute(
                "UPDATE violet_consumption_outbox SET response_json=?2
                WHERE request_id=?1 AND input_digest=?3 AND input_json=?4",
                params![job.request_id, response, job.input_digest, job.input_json],
            )?;
            if matching != 1
                || job.state != "dispatch_uncertain"
                || job.instance_id != current_instance
            {
                return Ok(false);
            }
            Ok(tx.execute(
                "UPDATE violet_body_permits SET state='armed'
                WHERE delivery_id=?1 AND registration_id=?2 AND delivery_generation=?3
                  AND instance_id=?4 AND request_id=?5 AND state='blocked'
                  AND EXISTS(SELECT 1 FROM violet_admissions a
                    WHERE a.prompt_id=violet_body_permits.prompt_id
                      AND a.registration_id=?2 AND a.delivery_generation=?3
                      AND a.ack_pending=0 AND a.consumption='queued'
                      AND a.factory_session=violet_body_permits.factory_session
                      AND a.recipient=violet_body_permits.recipient)",
                params![
                    job.delivery_id,
                    job.registration_id,
                    job.delivery_generation,
                    current_instance,
                    job.request_id
                ],
            )? == 1)
        })
    }

    /// Immutable requests survive process death and failed/lost acknowledgements.
    /// Background callers retry these exact bytes and UUIDs; an old instance's
    /// response can settle its outbox entry but can never arm the new instance.
    pub fn due_consumption_jobs(
        &self,
        registration: &str,
        project: &str,
        now_ms: i64,
    ) -> Result<Vec<VioletConsumptionJob>> {
        let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM violet_consumption_outbox
            WHERE registration_id=?1 AND project_id=?2 AND response_json IS NULL AND retry_at<=?3
            ORDER BY rowid LIMIT 25"
        ))?;
        Ok(stmt
            .query_map(params![registration, project, now_ms], job_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn defer_consumption_job(&self, job: &VioletConsumptionJob, retry_at: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
            conn.execute("UPDATE violet_consumption_outbox SET retry_at=?2,attempts=attempts+1
                WHERE request_id=?1 AND response_json IS NULL AND input_digest=?3 AND input_json=?4",
                params![job.request_id,retry_at,job.input_digest,job.input_json])?;
            Ok(())
        })
    }

    /// Snapshot local model-side surfacing separately from transport handoff.
    /// Queued and uncertain asks never manufacture consumed evidence.
    pub fn prepare_consumed_receipts(
        &self,
        registration: &str,
        project: &str,
        instance: &str,
        request_id: impl Fn() -> String,
    ) -> Result<usize> {
        crate::shared_db::with_immediate_write_txn_pooled(&self.queue.conn, |tx| {
            let rows: Vec<(String, i64)> = {
                let mut stmt = tx.prepare("SELECT a.delivery_id,a.delivery_generation FROM violet_admissions a
                    JOIN prompt_queue q ON q.id=a.prompt_id
                    WHERE a.registration_id=?1 AND a.project_id=?2 AND a.ack_pending=0
                      AND ((q.acked_at IS NOT NULL AND q.acked_via='explicit_ack') OR EXISTS(
                        SELECT 1 FROM prompt_queue_recipient_seen s WHERE s.prompt_id=q.id
                          AND s.recipient=a.recipient AND s.source IN ('hook_surfaced','inbox_poll','observed_wake')))
                      AND NOT EXISTS(SELECT 1 FROM violet_consumption_outbox o
                        WHERE o.delivery_id=a.delivery_id AND o.delivery_generation=a.delivery_generation AND o.state='consumed')
                    ORDER BY a.rowid LIMIT 25")?;
                stmt.query_map(params![registration, project], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (delivery, generation) in &rows {
                let id = request_id();
                let input = serde_json::json!({"schema":"cassy.coordination/v1", "request_id":id,
                    "actor":{"registration_id":registration},"delivery_generation":generation,
                    "state":"consumed","observed_at":Utc::now().to_rfc3339()})
                .to_string();
                let digest = format!("{:x}", Sha256::digest(input.as_bytes()));
                tx.execute("INSERT INTO violet_consumption_outbox
                    (request_id,delivery_id,registration_id,project_id,delivery_generation,instance_id,state,input_json,input_digest)
                    VALUES (?1,?2,?3,?4,?5,?6,'consumed',?7,?8)",
                    params![id,delivery,registration,project,generation,instance,input,digest])?;
                tx.execute(
                    "UPDATE violet_admissions SET consumption='consumed' WHERE delivery_id=?1",
                    [delivery],
                )?;
            }
            Ok(rows.len())
        })
    }

    /// A restarted daemon must revoke predecessor attempts before discovery.
    /// Recovery turns their remote uncertainty into one reference notice.
    pub fn revoke_predecessor_body_attempts(
        &self,
        registration: &str,
        instance: &str,
    ) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
            Ok(conn.execute(
                "UPDATE violet_body_permits SET state='revoked'
                WHERE registration_id=?1 AND instance_id<>?2 AND state IN ('blocked','armed')",
                params![registration, instance],
            )?)
        })
    }
}

/// Called under the same writer transaction that returns the body. Nothing,
/// including an unsuccessful transport, replenishes a spent original permit.
pub(super) fn spend_body_in_tx(
    tx: &Connection,
    prompt_id: i64,
    recipient: &str,
    session: Option<&str>,
) -> Result<bool> {
    let managed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM violet_body_permits WHERE prompt_id=?1)",
        [prompt_id],
        |r| r.get(0),
    )?;
    if !managed {
        return Ok(!tx
            .query_row(
                "SELECT prompt LIKE '<cas-violet-activity v=\"2\"%' FROM prompt_queue WHERE id=?1",
                [prompt_id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(true));
    }
    Ok(tx.execute(
        "UPDATE violet_body_permits SET state='spent'
        WHERE prompt_id=?1 AND recipient=?2 AND factory_session=?3 AND state='armed'
          AND EXISTS(SELECT 1 FROM prompt_queue q WHERE q.id=?1
            AND q.target=?2 AND q.factory_session=?3 AND q.processed_at IS NULL)",
        params![prompt_id, recipient, session],
    )? == 1)
}
