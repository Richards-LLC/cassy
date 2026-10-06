//! Atomic outbox repair for historical edges whose endpoint tasks never synced.
use crate::cloud::sync_queue::queue_ops::upsert_queue_row;
use crate::cloud::{EntityType, SyncOperation, SyncQueue};
use crate::error::CasError;
use crate::types::Task;
use rusqlite::{OptionalExtension, params};

// Only these explicit safety decisions are intentional parks. Unknown verdicts
// and registration conflicts remain failures, so reporting cannot hide them.
pub(super) const INTENTIONAL_PARK: &str = "COALESCE(last_outcome = 'parked' AND last_reason IN ('unattributed_origin', 'dependency_endpoint_unavailable', 'dependency_endpoint_deleted', 'dependency_endpoint_moved', 'dependency_endpoint_foreign'), 0)";

impl SyncQueue {
    pub(crate) fn park_intentionally(
        &self,
        id: i64,
        reason: &str,
        max_retries: i32,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue SET last_outcome='parked', last_reason=?2,
            last_error=?2, retry_count=MAX(retry_count,?3), failed_client_version=?4 WHERE id=?1",
            params![
                id,
                reason,
                max_retries,
                super::maintenance::recording_client_version()
            ],
        )?;
        Ok(())
    }

    /// Classify old client diagnostics even when every row is already terminal
    /// and the push has no pending work. Never infer an origin from this root.
    pub(crate) fn classify_unattributed_origin_parks(
        &self,
        team: &str,
        max_retries: i32,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        let rows = {
            let mut stmt=conn.prepare("SELECT id,entity_id,payload FROM sync_queue WHERE team_id=?1
                AND entity_type='entry' AND retry_count>=?2 AND last_error='row has no attributable origin_project'")?;
            stmt.query_map(params![team, max_retries], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        for (id, entity, payload) in rows {
            if Self::queued_origin(&conn, EntityType::Entry, &entity, payload.as_deref()).is_none()
            {
                conn.execute("UPDATE sync_queue SET last_outcome='parked',last_reason='unattributed_origin' WHERE id=?1",params![id])?;
            }
        }
        Ok(())
    }

    pub fn intentional_park_counts(
        &self,
        team: Option<&str>,
        max_retries: i32,
    ) -> Result<std::collections::BTreeMap<String, usize>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT last_reason,COUNT(*) FROM sync_queue WHERE retry_count>=?1
            AND team_id=?2 AND {INTENTIONAL_PARK} GROUP BY last_reason"
        ))?;
        let rows = stmt.query_map(params![max_retries, team.unwrap_or("")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Caller holds the task mutation lock while loading endpoint snapshots.
    /// Existing deletes, moves and newer task outbox writes always win. This
    /// transaction either stages the missing tasks and edge, or parks the edge.
    pub(crate) fn stage_healed_dependency(
        &self,
        id: &str,
        payload: &str,
        endpoints: &[Task],
        missing: &[Task],
        team: Option<&str>,
        project: &str,
        refusal: Option<&str>,
        max_retries: i32,
    ) -> Result<bool, CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let prior=tx.query_row("SELECT operation,last_reason,last_error FROM sync_queue
            WHERE entity_type='task_dependency' AND entity_id=?1 AND team_id=?2 AND project_id IS NULL",
            params![id,team.unwrap_or("")],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?))).optional()?;
        // A local edit/delete arriving after the pull read the queue wins.
        if prior.is_some_and(|(operation, reason, error)| {
            operation != "upsert"
                || reason.as_deref() != Some("orphan_dependency")
                || !error
                    .as_deref()
                    .is_some_and(|error| error.contains("orphan_dependency"))
        }) {
            return Ok(false);
        }
        let mut refusal = refusal.map(str::to_owned);
        for task in endpoints {
            let queued_delete: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_queue WHERE entity_type='task'
                AND entity_id=?1 AND team_id=?2 AND operation='delete')",
                params![task.id, team.unwrap_or("")],
                |r| r.get(0),
            )?;
            let moved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_queue WHERE entity_type='task'
                AND entity_id=?1 AND team_id=?2 AND project_id IS NOT NULL AND project_id!=?3)",
                params![task.id, team.unwrap_or(""), project],
                |r| r.get(0),
            )?;
            if queued_delete {
                refusal = Some("dependency_endpoint_deleted".into());
            } else if moved {
                refusal = Some("dependency_endpoint_moved".into());
            }
        }
        if refusal.is_none() {
            for task in missing {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_queue WHERE entity_type='task'
                    AND entity_id=?1 AND team_id=?2)",
                    params![task.id, team.unwrap_or("")],
                    |r| r.get(0),
                )?;
                if !exists {
                    let body =
                        serde_json::to_string(task).map_err(|e| CasError::Other(e.to_string()))?;
                    upsert_queue_row(
                        &tx,
                        EntityType::Task,
                        &task.id,
                        SyncOperation::Upsert,
                        Some(&body),
                        team.unwrap_or(""),
                        Some(project),
                    )?;
                }
            }
        }
        upsert_queue_row(
            &tx,
            EntityType::TaskDependency,
            id,
            SyncOperation::Upsert,
            Some(payload),
            team.unwrap_or(""),
            None,
        )?;
        // Clear a prior orphan verdict only after staging its prerequisites.
        tx.execute("UPDATE sync_queue SET last_outcome=?3,last_reason=?4,retry_count=?5,last_error=?4
            WHERE entity_type='task_dependency' AND entity_id=?1 AND team_id=?2 AND project_id IS NULL",
            params![id,team.unwrap_or(""),refusal.as_ref().map(|_|"parked"),refusal,if refusal.is_some(){max_retries}else{0}])?;
        tx.commit()?;
        Ok(refusal.is_none())
    }

    /// Tasks are pushed first. If an endpoint is still queued afterwards, its
    /// dependency must wait for acknowledgment, including terminal task errors.
    pub(crate) fn dependency_endpoint_queued(
        &self,
        from: &str,
        to: &str,
        team: &str,
    ) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_queue WHERE entity_type='task'
            AND entity_id IN (?1,?2) AND team_id=?3)",
            params![from, to, team],
            |r| r.get(0),
        )?)
    }
}
