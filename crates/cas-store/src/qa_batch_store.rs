//! Immutable, project-scoped staging QA receipts (GH #1203).
use crate::{Result, error::StoreError};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const QA_BATCH_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS qa_batch_receipts (
        id TEXT PRIMARY KEY, project_id TEXT NOT NULL, receipt_json TEXT NOT NULL,
        recorded_at TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS idx_qa_batch_receipts_project
        ON qa_batch_receipts(project_id, recorded_at DESC)",
    "CREATE TABLE IF NOT EXISTS qa_batch_deferrals (
        task_id TEXT PRIMARY KEY, delivered_head TEXT NOT NULL, deferred_at TEXT NOT NULL)",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaBatchReceipt {
    pub schema_version: u32,
    pub id: String,
    pub project_id: String,
    pub ledger_artifact_id: String,
    pub ledger_path: String,
    pub ledger_sha256: String,
    pub ledger_size_bytes: u64,
    pub staging_sha: String,
    /// Only PASS tasks, each bound to its own delivery anchor.
    pub task_heads: BTreeMap<String, String>,
    pub recorder_agent_id: String,
    pub recorded_at: String,
    /// The release-gate evidence pack this receipt attests (cas-e04d). The
    /// pack ledger is this ledger: absent on receipts without a pack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<QaBatchPack>,
}

/// A sealed `pack.json`, bound by its bytes and the gate verdict it states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaBatchPack {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    /// `CLEAN`, `FAILED` or `OWED`.
    pub gate: String,
}

fn connection(root: &Path) -> Result<std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>> {
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    {
        let guard = crate::shared_db::lock_connection(&conn)?;
        for sql in QA_BATCH_SCHEMA_STATEMENTS {
            guard.execute(sql, [])?;
        }
    }
    Ok(conn)
}

pub fn record_qa_batch_receipt(root: &Path, receipt: &QaBatchReceipt) -> Result<()> {
    if receipt.schema_version != 1
        || receipt.id.is_empty()
        || receipt.project_id.is_empty()
        || receipt.task_heads.is_empty()
        || receipt.ledger_size_bytes == 0
        || receipt.recorder_agent_id.is_empty()
    {
        return Err(StoreError::Parse("incomplete QA batch receipt".into()));
    }
    let json = serde_json::to_string(receipt).map_err(|e| StoreError::Parse(e.to_string()))?;
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    // INSERT only: corrections create a new receipt, never rewrite an attestation.
    guard.execute(
        "INSERT INTO qa_batch_receipts (id, project_id, receipt_json, recorded_at)
        VALUES (?1, ?2, ?3, ?4)",
        params![receipt.id, receipt.project_id, json, receipt.recorded_at],
    )?;
    Ok(())
}

pub fn list_qa_batch_receipts(root: &Path, project: &str) -> Result<Vec<QaBatchReceipt>> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let mut stmt = guard.prepare(
        "SELECT receipt_json FROM qa_batch_receipts
        WHERE project_id=?1 ORDER BY recorded_at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([project], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|json| serde_json::from_str(&json).map_err(|e| StoreError::Parse(e.to_string())))
        .collect()
}

/// A park creates an obligation that classification changes cannot erase.
pub fn defer_qa_batch_evidence(root: &Path, task: &str, head: &str) -> Result<()> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    guard.execute(
        "INSERT INTO qa_batch_deferrals (task_id, delivered_head, deferred_at)
        VALUES (?1, ?2, ?3) ON CONFLICT(task_id) DO UPDATE SET
        delivered_head=excluded.delivered_head, deferred_at=excluded.deferred_at",
        params![task, head, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn has_qa_batch_deferral(root: &Path, task: &str) -> Result<bool> {
    // Default/per-task closes inspect old debt too. Do not install batch
    // tables or write anything when checking a legacy database with no debt.
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let exists: bool = guard.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='qa_batch_deferrals')",
        [], |row| row.get(0))?;
    if !exists {
        return Ok(false);
    }
    Ok(guard.query_row(
        "SELECT EXISTS(SELECT 1 FROM qa_batch_deferrals WHERE task_id=?1)",
        [task],
        |row| row.get(0),
    )?)
}

pub fn qa_batch_receipts_for_task(
    root: &Path,
    project: &str,
    task: &str,
) -> Result<Vec<QaBatchReceipt>> {
    Ok(list_qa_batch_receipts(root, project)?
        .into_iter()
        .filter(|receipt| receipt.task_heads.contains_key(task))
        .collect())
}

/// Every recorded implementer identity, across all assignment/delivery cycles.
/// Read existing audit tables without installing schemas or truncating history.
/// Integration/verifier event actors are deliberately excluded: merging or
/// verifying another worker's delivery does not make someone its implementer.
pub fn qa_task_implementation_actors(root: &Path, task: &str) -> Result<Vec<String>> {
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let mut actors = std::collections::BTreeSet::new();
    for (table, columns) in [
        ("task_leases", &["agent_id"][..]),
        ("task_lease_history", &["agent_id", "previous_agent_id"][..]),
        (
            "worker_completion_receipts",
            &["worker_agent_id", "worker_name"][..],
        ),
        ("qa_passes", &["implementer_agent_id"][..]),
    ] {
        let exists: bool = guard.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        for column in columns {
            // Table/column identifiers above are compile-time constants.
            let mut stmt = guard.prepare(&format!(
                "SELECT DISTINCT {column} FROM {table} WHERE task_id=?1 AND {column} IS NOT NULL"
            ))?;
            for actor in stmt.query_map([task], |row| row.get::<_, String>(0))? {
                let actor = actor?;
                if !actor.trim().is_empty() {
                    actors.insert(actor);
                }
            }
        }
    }
    Ok(actors.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_batch_store_all_recorded_implementation_actors_cas_d697() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            qa_task_implementation_actors(root.path(), "cas-one")
                .unwrap()
                .is_empty()
        );
        let conn = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
        conn.execute_batch("CREATE TABLE task_lease_history(task_id TEXT, agent_id TEXT, previous_agent_id TEXT);
            CREATE TABLE worker_completion_receipts(task_id TEXT, worker_agent_id TEXT, worker_name TEXT);
            CREATE TABLE qa_passes(task_id TEXT, implementer_agent_id TEXT);").unwrap();
        conn.execute(
            "INSERT INTO task_lease_history VALUES ('cas-one','old-owner','transferred-from')",
            [],
        )
        .unwrap();
        for _ in 0..120 {
            conn.execute(
                "INSERT INTO task_lease_history VALUES ('cas-one','new-owner',NULL)",
                [],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO worker_completion_receipts VALUES ('cas-one','delivery-id','delivery-name')", []).unwrap();
        conn.execute(
            "INSERT INTO worker_completion_receipts VALUES ('cas-one','new-owner','new-owner')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO qa_passes VALUES ('cas-one','qa-implementer')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_lease_history VALUES ('cas-other','unrelated',NULL)",
            [],
        )
        .unwrap();
        assert_eq!(
            qa_task_implementation_actors(root.path(), "cas-one").unwrap(),
            vec![
                "delivery-id",
                "delivery-name",
                "new-owner",
                "old-owner",
                "qa-implementer",
                "transferred-from"
            ]
        );
    }

    fn receipt(id: &str, project: &str, tasks: &[&str]) -> QaBatchReceipt {
        QaBatchReceipt {
            schema_version: 1,
            id: id.into(),
            project_id: project.into(),
            ledger_artifact_id: "art-ledger".into(),
            ledger_path: "/ledger.md".into(),
            ledger_sha256: "a".repeat(64),
            ledger_size_bytes: 4,
            staging_sha: "b".repeat(40),
            recorder_agent_id: "supervisor".into(),
            recorded_at: Utc::now().to_rfc3339(),
            pack: None,
            task_heads: tasks
                .iter()
                .map(|id| ((*id).into(), "c".repeat(40)))
                .collect(),
        }
    }

    #[test]
    fn qa_batch_store_receipts_are_insert_only_cas_d697() {
        let root = tempfile::tempdir().unwrap();
        let original = receipt("qab-1", "project", &["cas-one"]);
        record_qa_batch_receipt(root.path(), &original).unwrap();
        let mut correction = original.clone();
        correction.staging_sha = "d".repeat(40);
        assert!(record_qa_batch_receipt(root.path(), &correction).is_err());
        assert_eq!(
            list_qa_batch_receipts(root.path(), "project").unwrap(),
            vec![original]
        );
    }

    #[test]
    fn qa_batch_store_lookup_scopes_project_and_task_cas_d697() {
        let root = tempfile::tempdir().unwrap();
        let first = receipt("qab-1", "project", &["cas-one", "cas-two"]);
        let second = receipt("qab-2", "project", &["cas-two"]);
        let foreign = receipt("qab-3", "other", &["cas-one"]);
        for receipt in [&first, &second, &foreign] {
            record_qa_batch_receipt(root.path(), receipt).unwrap();
        }
        assert_eq!(
            qa_batch_receipts_for_task(root.path(), "project", "cas-one").unwrap(),
            vec![first]
        );
        assert_eq!(
            qa_batch_receipts_for_task(root.path(), "project", "cas-two")
                .unwrap()
                .len(),
            2
        );
        assert!(
            qa_batch_receipts_for_task(root.path(), "project", "cas-missing")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            qa_batch_receipts_for_task(root.path(), "other", "cas-one").unwrap(),
            vec![foreign]
        );
    }

    #[test]
    fn qa_batch_store_debt_lookup_is_read_only_and_task_bound_cas_d697() {
        let root = tempfile::tempdir().unwrap();
        assert!(!has_qa_batch_deferral(root.path(), "cas-one").unwrap());
        let conn = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'qa_batch_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "default lookup must not create batch tables");
        defer_qa_batch_evidence(root.path(), "cas-one", "first-head").unwrap();
        defer_qa_batch_evidence(root.path(), "cas-one", "new-head").unwrap();
        assert!(has_qa_batch_deferral(root.path(), "cas-one").unwrap());
        assert!(!has_qa_batch_deferral(root.path(), "cas-two").unwrap());
        let head: String = conn
            .query_row(
                "SELECT delivered_head FROM qa_batch_deferrals WHERE task_id='cas-one'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(head, "new-head");
    }
}
