//! One store projection for doctor, factory session-end and the release fixture.
//! Transport is not surfacing; measurement never changes recipient_seen semantics.
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Result;

pub const METRICS_SQL: &str = include_str!("communication_slo/metrics.sql");
pub const SCHEMA_STATEMENTS: &[&str] = &[
    include_str!("communication_slo/schema-body.sql"),
    include_str!("communication_slo/schema-epoch.sql"),
    include_str!("communication_slo/schema-summaries.sql"),
];

pub(crate) const PRUNE_SCHEMA: &str = include_str!("communication_slo/schema-prune.sql");

pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    let tables: u32 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN
        ('communication_body_observations','communication_observation_epoch','communication_slo_summaries')",
        [], |row| row.get(0))?;
    if tables < 3 {
        for statement in SCHEMA_STATEMENTS {
            conn.execute_batch(statement)?;
        }
    }
    // Queue creation is lazy; this trigger belongs to queue initialization,
    // while m277 remains three replay-safe CREATE TABLE statements.
    conn.execute_batch(PRUNE_SCHEMA)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunicationSlos {
    pub factory_session: String,
    pub observed_at: String,
    pub false_abandon: u64,
    pub operator_events: u64,
    pub operator_receipts: u64,
    /// Local snapshots with no enrolled audience, excluded from the receipt
    /// denominator (cas-5c12). `None` marks a summary saved before that split,
    /// whose operator pair counted every local snapshot; it is shown as recorded.
    #[serde(default)]
    pub operator_unenrolled: Option<u64>,
    pub duplicate_body_reads: u64,
    pub director_nudges: u64,
    pub worker_hours: f64,
    pub gated_without_outcome: u64,
    pub body_observed_since: String,
    pub body_coverage_complete: bool,
}

impl CommunicationSlos {
    pub fn nudges_pass(&self) -> bool {
        // SQLite julianday arithmetic has sub-millisecond rounding error.
        self.director_nudges as f64 <= self.worker_hours + 1e-8
    }

    pub fn breach_count(&self) -> usize {
        [
            self.false_abandon != 0,
            self.operator_receipts != self.operator_events,
            self.duplicate_body_reads != 0,
            !self.nudges_pass(),
            self.gated_without_outcome != 0,
        ]
        .into_iter()
        .filter(|breach| *breach)
        .count()
    }

    pub fn outcome(&self) -> &'static str {
        if self.breach_count() > 0 {
            "BREACH"
        } else if !self.body_coverage_complete {
            "UNKNOWN"
        } else {
            "PASS"
        }
    }

    /// Outcome first, explicit values/targets; ASCII works on every terminal.
    pub fn lines(&self) -> Vec<String> {
        fn verdict(pass: bool) -> &'static str {
            if pass { "PASS" } else { "BREACH" }
        }
        let receipts = if self.operator_events == 0 {
            100.0
        } else {
            self.operator_receipts as f64 * 100.0 / self.operator_events as f64
        };
        let nudge_rate = if self.worker_hours > 0.0 {
            format!(
                "{:.3}/hour",
                self.director_nudges as f64 / self.worker_hours
            )
        } else if self.director_nudges == 0 {
            "0/hour".into()
        } else {
            "unbounded/hour".into()
        };
        let duplicate_outcome = if self.duplicate_body_reads != 0 {
            "BREACH"
        } else if self.body_coverage_complete {
            "PASS"
        } else {
            "UNKNOWN"
        };
        let relay = match self.operator_unenrolled {
            Some(unenrolled) if self.operator_events == 0 && unenrolled > 0 => format!(
                "N/A operator relay receipts: not applicable, {unenrolled} unenrolled local \
                 snapshot(s); `cas hub operator bind` enrolls this project (target 100%)"
            ),
            unenrolled => format!(
                "{} operator relay receipts: {receipts:.1}% ({}/{}, target 100%{})",
                verdict(self.operator_receipts == self.operator_events),
                self.operator_receipts,
                self.operator_events,
                match unenrolled {
                    None => "; legacy denominator: all local snapshots".to_string(),
                    Some(0) if self.operator_events == 0 => "; no events".to_string(),
                    Some(0) => String::new(),
                    Some(n) => format!("; {n} unenrolled excluded"),
                }
            ),
        };
        vec![
            format!(
                "{} false-abandon: {} (target 0)",
                verdict(self.false_abandon == 0),
                self.false_abandon
            ),
            relay,
            format!(
                "{duplicate_outcome} duplicate body reads: {} (target 0)",
                self.duplicate_body_reads
            ),
            format!(
                "{} director nudges: {nudge_rate} (target <=1/hour)",
                verdict(self.nudges_pass())
            ),
            format!(
                "{} gated-without-outcome: {} (target 0)",
                verdict(self.gated_without_outcome == 0),
                self.gated_without_outcome
            ),
        ]
    }

    pub fn message(&self) -> String {
        let mut lines = vec![format!(
            "{} communication: {} breach(es); session {}",
            self.outcome(),
            self.breach_count(),
            self.factory_session
        )];
        lines.extend(self.lines());
        lines.push(format!(
            "Nudges: {} / {:.3} worker-hours",
            self.director_nudges, self.worker_hours
        ));
        if !self.body_coverage_complete {
            lines.push(format!(
                "Body coverage starts {}; earlier reads unknown",
                self.body_observed_since
            ));
        }
        lines.join("\n")
    }
}

fn measure(conn: &Connection, session: &str, now: &str) -> Result<CommunicationSlos> {
    Ok(conn.query_row(METRICS_SQL, params![session, now], |row| {
        Ok(CommunicationSlos {
            factory_session: session.to_string(),
            observed_at: now.to_string(),
            false_abandon: row.get::<_, i64>("false_abandon")? as u64,
            operator_events: row.get::<_, i64>("operator_events")? as u64,
            operator_receipts: row.get::<_, i64>("operator_receipts")? as u64,
            operator_unenrolled: Some(row.get::<_, i64>("operator_unenrolled")? as u64),
            duplicate_body_reads: row.get::<_, i64>("duplicate_body_reads")? as u64,
            director_nudges: row.get::<_, i64>("director_nudges")? as u64,
            worker_hours: row.get("worker_hours")?,
            gated_without_outcome: row.get::<_, i64>("gated_without_outcome")? as u64,
            body_observed_since: row.get("body_observed_since")?,
            body_coverage_complete: row.get("body_coverage_complete")?,
        })
    })?)
}

fn reader(cas_dir: &Path) -> Result<Connection> {
    let conn =
        Connection::open_with_flags(cas_dir.join("cas.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    crate::shared_db::install_busy_handler(&conn)?;
    Ok(conn)
}

pub fn read_session(
    cas_dir: &Path,
    session: &str,
    now: DateTime<Utc>,
) -> Result<CommunicationSlos> {
    measure(&reader(cas_dir)?, session, &now.to_rfc3339())
}

/// The most recently started factory session, including retired/idle sessions.
/// Ended sessions keep the exact snapshot written at shutdown.
pub fn last_session(cas_dir: &Path, now: DateTime<Utc>) -> Result<Option<CommunicationSlos>> {
    let conn = reader(cas_dir)?;
    let tx = conn.unchecked_transaction()?;
    // Fresh projects create prompt_queue lazily on their first message.
    let queue_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='prompt_queue')",
        [],
        |row| row.get(0),
    )?;
    if !queue_exists {
        return Ok(None);
    }
    let session: Option<String> = tx
        .query_row(
            "WITH sessions AS (
          SELECT factory_session, created_at at FROM prompt_queue
          UNION ALL SELECT factory_session, created_at FROM operator_delivery_outbox
          UNION ALL SELECT factory_session, registered_at FROM agents
          UNION ALL SELECT factory_session, observed_at FROM communication_slo_summaries
          UNION ALL SELECT json_extract(metadata,'$.retired_agent.factory_session'),
             json_extract(metadata,'$.retired_agent.registered_at') FROM events
             WHERE event_type='agent_shutdown' AND json_valid(metadata)
        ) SELECT factory_session FROM sessions WHERE factory_session IS NOT NULL
          GROUP BY factory_session ORDER BY MIN(julianday(at)) DESC, factory_session DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(session) = session else {
        return Ok(None);
    };
    let saved: Option<String> = tx
        .query_row(
            "SELECT summary_json FROM communication_slo_summaries WHERE factory_session=?",
            [&session],
            |row| row.get(0),
        )
        .optional()?;
    match saved {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(Some(measure(&tx, &session, &now.to_rfc3339())?)),
    }
}

/// Called off the daemon loop after agent cleanup; retirement events retain exposure.
pub fn write_session_end(
    cas_dir: &Path,
    session: &str,
    now: DateTime<Utc>,
) -> Result<CommunicationSlos> {
    let conn =
        Connection::open_with_flags(cas_dir.join("cas.db"), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::shared_db::install_busy_handler(&conn)?;
    crate::shared_db::with_write_retry(|| {
        let tx = crate::shared_db::ImmediateTx::new(&conn)?;
        let metrics = measure(&tx, session, &now.to_rfc3339())?;
        tx.execute(
            "INSERT INTO communication_slo_summaries(factory_session,observed_at,summary_json)
          VALUES(?1,?2,?3) ON CONFLICT(factory_session) DO UPDATE SET
          observed_at=excluded.observed_at,summary_json=excluded.summary_json",
            params![
                session,
                metrics.observed_at,
                serde_json::to_string(&metrics)?
            ],
        )?;
        tx.commit()?;
        Ok(metrics)
    })
}

/// Count one actual body returned by a drain, in that drain's transaction.
/// Re-observing one wake is bookkeeping; callers admit it only on receipt advance.
pub(crate) fn observe_body(conn: &Connection, id: i64, recipient: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO communication_body_observations(prompt_id,recipient,body_count)
        VALUES(?1,?2,1) ON CONFLICT(prompt_id,recipient) DO UPDATE SET body_count=body_count+1",
        params![id, recipient],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
