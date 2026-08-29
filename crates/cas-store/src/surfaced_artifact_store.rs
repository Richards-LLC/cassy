//! Durable context-injection records and artifact impact aggregates.

use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::StoreError;
use crate::shared_db::ImmediateTx;
use crate::{Result, shared_db};

/// SQLite DDL for the append-only session-to-artifact injection ledger.
///
/// The table intentionally does not foreign-key artifact IDs: retired or
/// deleted rules and skills still need their historical impact rows.
pub const SURFACED_ARTIFACT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS surfaced_artifacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    artifact_type TEXT NOT NULL CHECK (artifact_type IN ('rule', 'skill')),
    artifact_preview TEXT,
    surfaced_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_surfaced_artifacts_session
    ON surfaced_artifacts(session_id, surfaced_at);
CREATE INDEX IF NOT EXISTS idx_surfaced_artifacts_artifact
    ON surfaced_artifacts(artifact_type, artifact_id, surfaced_at);
"#;

/// Statement-level form used by the numbered migration runner.
pub const SURFACED_ARTIFACT_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS surfaced_artifacts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        artifact_type TEXT NOT NULL CHECK (artifact_type IN ('rule', 'skill')),
        artifact_preview TEXT,
        surfaced_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_surfaced_artifacts_session
        ON surfaced_artifacts(session_id, surfaced_at)",
    "CREATE INDEX IF NOT EXISTS idx_surfaced_artifacts_artifact
        ON surfaced_artifacts(artifact_type, artifact_id, surfaced_at)",
];

/// One artifact selected for context injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfacedArtifact {
    pub artifact_id: String,
    pub artifact_type: String,
    pub preview: Option<String>,
}

/// Impact summary for one injected rule or skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacedArtifactImpact {
    pub artifact_id: String,
    pub artifact_type: String,
    pub surfaced_count: u64,
    pub session_count: u64,
    /// Counts distinct sessions by their eventual `sessions.outcome`.
    pub outcome_counts: BTreeMap<String, u64>,
    /// Rule feedback counters. Skills currently expose usage_count instead;
    /// their feedback fields remain zero until the skill feedback contract is
    /// added, rather than treating usage as unverified helpfulness.
    pub helpful_count: u64,
    pub harmful_count: u64,
    pub usage_count: u64,
}

/// SQLite store for batched SessionStart surface writes and impact reports.
pub struct SqliteSurfacedArtifactStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SqliteSurfacedArtifactStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute_batch(SURFACED_ARTIFACT_SCHEMA)?;
        Ok(())
    }

    /// Persist all artifacts from one context build in one write transaction.
    /// Rule counters are updated in the same transaction as their audit rows,
    /// avoiding one SQLite write/fsync per injected rule.
    pub fn record_batch(&self, session_id: &str, artifacts: &[SurfacedArtifact]) -> Result<usize> {
        if session_id.trim().is_empty() {
            return Err(StoreError::Parse(
                "surface records require a non-empty session ID".to_string(),
            ));
        }
        for artifact in artifacts {
            if !matches!(artifact.artifact_type.as_str(), "rule" | "skill") {
                return Err(StoreError::Parse(format!(
                    "unsupported surfaced artifact type: {}",
                    artifact.artifact_type
                )));
            }
            if artifact.artifact_id.trim().is_empty() {
                return Err(StoreError::Parse(
                    "surface records require a non-empty artifact ID".to_string(),
                ));
            }
        }
        if artifacts.is_empty() {
            return Ok(0);
        }

        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = ImmediateTx::new(&conn)?;
        let surfaced_at = Utc::now().to_rfc3339();
        for artifact in artifacts {
            tx.execute(
                "INSERT INTO surfaced_artifacts
                 (session_id, artifact_id, artifact_type, artifact_preview, surfaced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    session_id,
                    artifact.artifact_id,
                    artifact.artifact_type,
                    artifact.preview,
                    surfaced_at,
                ],
            )?;
            if artifact.artifact_type == "rule" {
                tx.execute(
                    "UPDATE rules
                     SET surface_count = MIN(surface_count + 1, 2147483647)
                     WHERE id = ?1",
                    params![artifact.artifact_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(artifacts.len())
    }

    /// Return the number of persisted rows for one session.
    pub fn count_for_session(&self, session_id: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.query_row(
            "SELECT COUNT(*) FROM surfaced_artifacts WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as u64)
        .map_err(StoreError::Database)
    }

    /// Aggregate injected artifacts and join each row to its session outcome.
    pub fn aggregate(&self, limit: usize) -> Result<Vec<SurfacedArtifactImpact>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT sa.artifact_id, sa.artifact_type,
                    COUNT(*) AS surfaced_count,
                    COUNT(DISTINCT sa.session_id) AS session_count,
                    CASE WHEN sa.artifact_type = 'rule'
                         THEN COALESCE(MAX(r.helpful_count), 0) ELSE 0 END,
                    CASE WHEN sa.artifact_type = 'rule'
                         THEN COALESCE(MAX(r.harmful_count), 0) ELSE 0 END,
                    CASE WHEN sa.artifact_type = 'skill'
                         THEN COALESCE(MAX(sk.usage_count), 0) ELSE 0 END
             FROM surfaced_artifacts sa
             LEFT JOIN rules r
               ON sa.artifact_type = 'rule' AND r.id = sa.artifact_id
             LEFT JOIN skills sk
               ON sa.artifact_type = 'skill' AND sk.id = sa.artifact_id
             GROUP BY sa.artifact_id, sa.artifact_type
             ORDER BY surfaced_count DESC, sa.artifact_type, sa.artifact_id
             LIMIT ?1",
        )?;
        let mut impacts = stmt
            .query_map(params![limit as i64], |row| {
                Ok(SurfacedArtifactImpact {
                    artifact_id: row.get(0)?,
                    artifact_type: row.get(1)?,
                    surfaced_count: row.get::<_, i64>(2)?.max(0) as u64,
                    session_count: row.get::<_, i64>(3)?.max(0) as u64,
                    outcome_counts: BTreeMap::new(),
                    helpful_count: row.get::<_, i64>(4)?.max(0) as u64,
                    harmful_count: row.get::<_, i64>(5)?.max(0) as u64,
                    usage_count: row.get::<_, i64>(6)?.max(0) as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let impact_indexes: HashMap<(String, String), usize> = impacts
            .iter()
            .enumerate()
            .map(|(index, impact)| {
                (
                    (impact.artifact_id.clone(), impact.artifact_type.clone()),
                    index,
                )
            })
            .collect();

        let mut outcome_stmt = conn.prepare(
            "SELECT sa.artifact_id, sa.artifact_type, s.outcome,
                    COUNT(DISTINCT sa.session_id)
             FROM surfaced_artifacts sa
             JOIN sessions s ON s.session_id = sa.session_id
             WHERE s.outcome IS NOT NULL
             GROUP BY sa.artifact_id, sa.artifact_type, s.outcome",
        )?;
        let outcome_rows = outcome_stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?.max(0) as u64,
            ))
        })?;
        for row in outcome_rows {
            let (artifact_id, artifact_type, outcome, count) = row?;
            if let Some(index) = impact_indexes.get(&(artifact_id, artifact_type)) {
                impacts[*index].outcome_counts.insert(outcome, count);
            }
        }
        Ok(impacts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RuleStore, SkillStore, SqliteRuleStore, SqliteStore, Store};
    use cas_types::{Rule, RuleStatus, Session, SessionOutcome, Skill, SkillStatus};
    use tempfile::TempDir;

    fn surfaced(artifact_id: &str, artifact_type: &str) -> SurfacedArtifact {
        SurfacedArtifact {
            artifact_id: artifact_id.to_string(),
            artifact_type: artifact_type.to_string(),
            preview: Some(format!("preview for {artifact_id}")),
        }
    }

    #[test]
    fn records_surfaces_and_batches_rule_counter_updates() {
        let temp = TempDir::new().unwrap();
        let rule_store = SqliteRuleStore::open(temp.path()).unwrap();
        rule_store.init().unwrap();
        let mut rule = Rule::new("rule-1".to_string(), "Always test".to_string());
        rule.status = RuleStatus::Proven;
        rule_store.add(&rule).unwrap();

        let store = SqliteSurfacedArtifactStore::open(temp.path()).unwrap();
        let written = store
            .record_batch(
                "session-1",
                &[surfaced("rule-1", "rule"), surfaced("skill-1", "skill")],
            )
            .unwrap();

        assert_eq!(written, 2);
        assert_eq!(rule_store.get("rule-1").unwrap().surface_count, 1);
        assert_eq!(store.count_for_session("session-1").unwrap(), 2);
    }

    #[test]
    fn impact_aggregate_joins_surface_rows_to_session_outcomes_and_feedback() {
        let temp = TempDir::new().unwrap();
        let entry_store = SqliteStore::open(temp.path()).unwrap();
        entry_store.init().unwrap();
        let rule_store = SqliteRuleStore::open(temp.path()).unwrap();
        rule_store.init().unwrap();

        let mut rule = Rule::new("rule-1".to_string(), "Always test".to_string());
        rule.status = RuleStatus::Proven;
        rule.helpful_count = 3;
        rule.harmful_count = 1;
        rule_store.add(&rule).unwrap();

        let mut skill = Skill::new("skill-1".to_string(), "Test skill".to_string());
        skill.description = "Run tests".to_string();
        skill.status = SkillStatus::Enabled;
        skill.usage_count = 2;
        let skill_store = crate::SqliteSkillStore::open(temp.path()).unwrap();
        skill_store.init().unwrap();
        skill_store.add(&skill).unwrap();

        let completed = Session::new("session-completed".to_string(), "/repo".to_string(), None);
        entry_store.start_session(&completed).unwrap();
        entry_store
            .update_session_outcome("session-completed", SessionOutcome::TasksCompleted)
            .unwrap();
        let abandoned = Session::new("session-abandoned".to_string(), "/repo".to_string(), None);
        entry_store.start_session(&abandoned).unwrap();
        entry_store
            .update_session_outcome("session-abandoned", SessionOutcome::Abandoned)
            .unwrap();

        let store = SqliteSurfacedArtifactStore::open(temp.path()).unwrap();
        store
            .record_batch(
                "session-completed",
                &[surfaced("rule-1", "rule"), surfaced("skill-1", "skill")],
            )
            .unwrap();
        store
            .record_batch("session-abandoned", &[surfaced("rule-1", "rule")])
            .unwrap();

        let impacts = store.aggregate(10).unwrap();
        let rule_impact = impacts
            .iter()
            .find(|impact| impact.artifact_id == "rule-1")
            .unwrap();
        assert_eq!(rule_impact.surfaced_count, 2);
        assert_eq!(rule_impact.session_count, 2);
        assert_eq!(rule_impact.outcome_counts["tasks_completed"], 1);
        assert_eq!(rule_impact.outcome_counts["abandoned"], 1);
        assert_eq!(rule_impact.helpful_count, 3);
        assert_eq!(rule_impact.harmful_count, 1);

        let skill_impact = impacts
            .iter()
            .find(|impact| impact.artifact_id == "skill-1")
            .unwrap();
        assert_eq!(skill_impact.surfaced_count, 1);
        assert_eq!(skill_impact.session_count, 1);
        assert_eq!(skill_impact.outcome_counts["tasks_completed"], 1);
        assert_eq!(skill_impact.usage_count, 2);
    }
}
