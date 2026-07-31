//! Durable retrieval query identity and explicit outcome feedback.
//!
//! This store is intentionally observational. Nothing in this module updates
//! entry/rule helpful counters, search weights, or any other ranking input.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::StoreError;
use crate::shared_db::ImmediateTx;
use crate::{Result, shared_db};

/// Identifier for the unchanged ranking policy observed by this foundation.
pub const DEFAULT_RETRIEVAL_POLICY: &str = "current-default-v1";

/// Canonical schema for versioned retrieval identities and explicit outcomes.
pub const RETRIEVAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS retrieval_queries (
    id TEXT PRIMARY KEY,
    query_fingerprint TEXT NOT NULL,
    query_family TEXT NOT NULL,
    ranking_policy TEXT NOT NULL,
    session_hash TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS retrieval_query_results (
    query_id TEXT NOT NULL,
    result_id TEXT NOT NULL,
    document_type TEXT NOT NULL,
    rank INTEGER NOT NULL,
    PRIMARY KEY (query_id, result_id),
    FOREIGN KEY (query_id) REFERENCES retrieval_queries(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS retrieval_outcomes (
    id TEXT PRIMARY KEY,
    query_id TEXT NOT NULL,
    result_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (
        outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful')
    ),
    actor_hash TEXT NOT NULL,
    session_hash TEXT NOT NULL,
    correction_ref TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (query_id, result_id)
        REFERENCES retrieval_query_results(query_id, result_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_retrieval_queries_family
    ON retrieval_queries(query_family, created_at);
CREATE INDEX IF NOT EXISTS idx_retrieval_results_type
    ON retrieval_query_results(document_type, query_id);
CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_query
    ON retrieval_outcomes(query_id, result_id);
CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_created
    ON retrieval_outcomes(created_at);
"#;

/// Statement-level form used by the numbered migration runner, which invokes
/// `Connection::execute` once per item rather than `execute_batch`.
///
/// Keep this in lockstep with [`RETRIEVAL_SCHEMA`]; the migration shape test
/// below compares the two forms.
pub const RETRIEVAL_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS retrieval_queries (
        id TEXT PRIMARY KEY,
        query_fingerprint TEXT NOT NULL,
        query_family TEXT NOT NULL,
        ranking_policy TEXT NOT NULL,
        session_hash TEXT,
        created_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS retrieval_query_results (
        query_id TEXT NOT NULL,
        result_id TEXT NOT NULL,
        document_type TEXT NOT NULL,
        rank INTEGER NOT NULL,
        PRIMARY KEY (query_id, result_id),
        FOREIGN KEY (query_id) REFERENCES retrieval_queries(id) ON DELETE CASCADE
    )",
    "CREATE TABLE IF NOT EXISTS retrieval_outcomes (
        id TEXT PRIMARY KEY,
        query_id TEXT NOT NULL,
        result_id TEXT NOT NULL,
        outcome TEXT NOT NULL CHECK (
            outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful')
        ),
        actor_hash TEXT NOT NULL,
        session_hash TEXT NOT NULL,
        correction_ref TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY (query_id, result_id)
            REFERENCES retrieval_query_results(query_id, result_id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_retrieval_queries_family
        ON retrieval_queries(query_family, created_at)",
    "CREATE INDEX IF NOT EXISTS idx_retrieval_results_type
        ON retrieval_query_results(document_type, query_id)",
    "CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_query
        ON retrieval_outcomes(query_id, result_id)",
    "CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_created
        ON retrieval_outcomes(created_at)",
];

/// Stable explicit outcomes accepted by the feedback API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetrievalOutcome {
    Used,
    Helpful,
    Ignored,
    Corrected,
    Harmful,
}

impl RetrievalOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Used => "used",
            Self::Helpful => "helpful",
            Self::Ignored => "ignored",
            Self::Corrected => "corrected",
            Self::Harmful => "harmful",
        }
    }
}

impl FromStr for RetrievalOutcome {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "used" => Ok(Self::Used),
            "helpful" => Ok(Self::Helpful),
            "ignored" => Ok(Self::Ignored),
            "corrected" => Ok(Self::Corrected),
            "harmful" => Ok(Self::Harmful),
            other => Err(StoreError::Parse(format!(
                "invalid retrieval outcome '{other}'; expected used, helpful, ignored, corrected, or harmful"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalHitIdentity {
    pub result_id: String,
    pub document_type: String,
    pub rank: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalQuery {
    pub id: String,
    pub query_fingerprint: String,
    pub query_family: String,
    pub ranking_policy: String,
    pub session_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalOutcomeEvent {
    pub id: String,
    pub query_id: String,
    pub result_id: String,
    pub outcome: RetrievalOutcome,
    pub actor_hash: String,
    pub session_hash: String,
    pub correction_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Offline quality aggregate. Rates use all explicit outcomes as denominator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalAggregate {
    pub document_type: String,
    pub query_family: String,
    pub ranking_policy: String,
    pub total: u64,
    pub used: u64,
    pub helpful: u64,
    pub ignored: u64,
    pub corrected: u64,
    pub harmful: u64,
    pub usefulness_rate: f64,
    pub ignore_rate: f64,
    pub correction_rate: f64,
}

pub trait RetrievalStore: Send + Sync {
    fn init(&self) -> Result<()>;

    fn record_query(
        &self,
        id: &str,
        raw_query: &str,
        query_family: &str,
        ranking_policy: &str,
        session_id: Option<&str>,
        hits: &[RetrievalHitIdentity],
    ) -> Result<RetrievalQuery>;

    fn record_outcome(
        &self,
        id: &str,
        query_id: &str,
        result_id: &str,
        outcome: RetrievalOutcome,
        actor_id: &str,
        session_id: &str,
        correction_ref: Option<&str>,
    ) -> Result<RetrievalOutcomeEvent>;

    fn aggregate(&self) -> Result<Vec<RetrievalAggregate>>;
}

pub struct SqliteRetrievalStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRetrievalStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    fn fingerprint(domain: &str, value: &str) -> String {
        let normalized = value
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
            .join(" ");
        let digest = Sha256::digest(format!("{domain}\0{normalized}").as_bytes());
        format!("sha256:{digest:x}")
    }

    fn identity_hash(domain: &str, value: &str) -> Result<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(StoreError::Parse(
                "retrieval actor/session identity cannot be empty".to_string(),
            ));
        }
        Ok(Self::fingerprint(domain, trimmed))
    }

    fn validate_opaque_reference(value: &str) -> Result<()> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(StoreError::Parse(
                "correction_ref must be a 1-128 character opaque ID using only letters, digits, '.', '-', or '_'"
                    .to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn get_query(&self, id: &str) -> Result<Option<RetrievalQuery>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.query_row(
            "SELECT id, query_fingerprint, query_family, ranking_policy, session_hash, created_at
             FROM retrieval_queries WHERE id = ?1",
            [id],
            |row| {
                let created: String = row.get(5)?;
                Ok(RetrievalQuery {
                    id: row.get(0)?,
                    query_fingerprint: row.get(1)?,
                    query_family: row.get(2)?,
                    ranking_policy: row.get(3)?,
                    session_hash: row.get(4)?,
                    created_at: DateTime::parse_from_rfc3339(&created)
                        .map(|value| value.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                })
            },
        )
        .optional()
        .map_err(StoreError::Database)
    }
}

impl RetrievalStore for SqliteRetrievalStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute_batch(RETRIEVAL_SCHEMA)?;
        Ok(())
    }

    fn record_query(
        &self,
        id: &str,
        raw_query: &str,
        query_family: &str,
        ranking_policy: &str,
        session_id: Option<&str>,
        hits: &[RetrievalHitIdentity],
    ) -> Result<RetrievalQuery> {
        let created_at = Utc::now();
        let query = RetrievalQuery {
            id: id.to_string(),
            // Salt each fingerprint with the random query identity. This keeps
            // low-entropy queries from being recoverable with a reusable
            // dictionary while preserving a durable integrity fingerprint.
            query_fingerprint: Self::fingerprint(&format!("query:{id}"), raw_query),
            query_family: query_family.to_string(),
            ranking_policy: ranking_policy.to_string(),
            session_hash: session_id
                .filter(|value| !value.trim().is_empty())
                .map(|value| Self::identity_hash("session", value))
                .transpose()?,
            created_at,
        };

        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = ImmediateTx::new(&conn)?;
        tx.execute(
            "INSERT INTO retrieval_queries
             (id, query_fingerprint, query_family, ranking_policy, session_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                query.id,
                query.query_fingerprint,
                query.query_family,
                query.ranking_policy,
                query.session_hash,
                query.created_at.to_rfc3339(),
            ],
        )?;
        for hit in hits {
            tx.execute(
                "INSERT INTO retrieval_query_results
                 (query_id, result_id, document_type, rank)
                 VALUES (?1, ?2, ?3, ?4)",
                params![query.id, hit.result_id, hit.document_type, hit.rank as i64],
            )?;
        }
        tx.commit()?;
        Ok(query)
    }

    fn record_outcome(
        &self,
        id: &str,
        query_id: &str,
        result_id: &str,
        outcome: RetrievalOutcome,
        actor_id: &str,
        session_id: &str,
        correction_ref: Option<&str>,
    ) -> Result<RetrievalOutcomeEvent> {
        if let Some(reference) = correction_ref {
            Self::validate_opaque_reference(reference)?;
        }
        if outcome == RetrievalOutcome::Corrected && correction_ref.is_none() {
            return Err(StoreError::Parse(
                "corrected retrieval outcomes require correction_ref".to_string(),
            ));
        }

        let event = RetrievalOutcomeEvent {
            id: id.to_string(),
            query_id: query_id.to_string(),
            result_id: result_id.to_string(),
            outcome,
            actor_hash: Self::identity_hash("actor", actor_id)?,
            session_hash: Self::identity_hash("session", session_id)?,
            correction_ref: correction_ref.map(str::to_string),
            created_at: Utc::now(),
        };

        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM retrieval_query_results
                 WHERE query_id = ?1 AND result_id = ?2",
                params![query_id, result_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Err(StoreError::NotFound(format!(
                "retrieval result {result_id} in query {query_id}"
            )));
        }

        conn.execute(
            "INSERT INTO retrieval_outcomes
             (id, query_id, result_id, outcome, actor_hash, session_hash, correction_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.id,
                event.query_id,
                event.result_id,
                event.outcome.as_str(),
                event.actor_hash,
                event.session_hash,
                event.correction_ref,
                event.created_at.to_rfc3339(),
            ],
        )?;
        Ok(event)
    }

    fn aggregate(&self) -> Result<Vec<RetrievalAggregate>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT r.document_type, q.query_family, q.ranking_policy,
                    COUNT(*) AS total,
                    SUM(CASE WHEN o.outcome = 'used' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN o.outcome = 'helpful' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN o.outcome = 'ignored' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN o.outcome = 'corrected' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN o.outcome = 'harmful' THEN 1 ELSE 0 END)
             FROM retrieval_outcomes o
             JOIN retrieval_queries q ON q.id = o.query_id
             JOIN retrieval_query_results r
               ON r.query_id = o.query_id AND r.result_id = o.result_id
             GROUP BY r.document_type, q.query_family, q.ranking_policy
             ORDER BY r.document_type, q.query_family, q.ranking_policy",
        )?;
        let rows = stmt.query_map([], |row| {
            let total = row.get::<_, i64>(3)?.max(0) as u64;
            let used = row.get::<_, i64>(4)?.max(0) as u64;
            let helpful = row.get::<_, i64>(5)?.max(0) as u64;
            let ignored = row.get::<_, i64>(6)?.max(0) as u64;
            let corrected = row.get::<_, i64>(7)?.max(0) as u64;
            let harmful = row.get::<_, i64>(8)?.max(0) as u64;
            let denominator = total.max(1) as f64;
            Ok(RetrievalAggregate {
                document_type: row.get(0)?,
                query_family: row.get(1)?,
                ranking_policy: row.get(2)?,
                total,
                used,
                helpful,
                ignored,
                corrected,
                harmful,
                usefulness_rate: (used + helpful) as f64 / denominator,
                ignore_rate: ignored as f64 / denominator,
                correction_rate: corrected as f64 / denominator,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn hit(id: &str, document_type: &str, rank: usize) -> RetrievalHitIdentity {
        RetrievalHitIdentity {
            result_id: id.to_string(),
            document_type: document_type.to_string(),
            rank,
        }
    }

    #[test]
    fn persists_hashed_query_and_identity_without_raw_payloads() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_query(
                "qry-1",
                "Secret customer query",
                "keyword",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session-private"),
                &[hit("entry-1", "entry", 0)],
            )
            .unwrap();
        let query = store.get_query("qry-1").unwrap().unwrap();
        assert!(query.query_fingerprint.starts_with("sha256:"));
        assert!(!query.query_fingerprint.contains("customer"));
        assert_ne!(query.session_hash.as_deref(), Some("session-private"));
        store
            .record_query(
                "qry-2",
                "Secret customer query",
                "keyword",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session-private"),
                &[],
            )
            .unwrap();
        let second_query = store.get_query("qry-2").unwrap().unwrap();
        assert_ne!(
            query.query_fingerprint, second_query.query_fingerprint,
            "per-query salt must prevent reusable low-entropy query hashes"
        );

        let event = store
            .record_outcome(
                "out-1",
                "qry-1",
                "entry-1",
                RetrievalOutcome::Helpful,
                "private-actor",
                "session-private",
                None,
            )
            .unwrap();
        assert_ne!(event.actor_hash, "private-actor");
        assert_ne!(event.session_hash, "session-private");

        let db = std::fs::read(temp.path().join("cas.db")).unwrap();
        let raw = String::from_utf8_lossy(&db);
        assert!(!raw.contains("Secret customer query"));
        assert!(!raw.contains("private-actor"));
        assert!(!raw.contains("session-private"));
    }

    #[test]
    fn rejects_unrecorded_results_and_unsafe_correction_references() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_query(
                "qry-1",
                "query",
                "keyword",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[hit("entry-1", "entry", 0)],
            )
            .unwrap();

        assert!(
            store
                .record_outcome(
                    "out-1",
                    "qry-1",
                    "entry-missing",
                    RetrievalOutcome::Ignored,
                    "actor",
                    "session",
                    None,
                )
                .is_err()
        );
        assert!(
            store
                .record_outcome(
                    "out-2",
                    "qry-1",
                    "entry-1",
                    RetrievalOutcome::Corrected,
                    "actor",
                    "session",
                    Some("/home/user/private.txt"),
                )
                .is_err()
        );
    }

    #[test]
    fn aggregates_explicit_outcomes_by_document_type_and_query_family() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_query(
                "qry-1",
                "query",
                "keyword",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[hit("entry-1", "entry", 0), hit("entry-2", "entry", 1)],
            )
            .unwrap();
        for (id, result, outcome, correction) in [
            ("out-1", "entry-1", RetrievalOutcome::Used, None),
            ("out-2", "entry-1", RetrievalOutcome::Helpful, None),
            ("out-3", "entry-2", RetrievalOutcome::Ignored, None),
            (
                "out-4",
                "entry-2",
                RetrievalOutcome::Corrected,
                Some("entry-3"),
            ),
        ] {
            store
                .record_outcome(id, "qry-1", result, outcome, "actor", "session", correction)
                .unwrap();
        }

        let aggregates = store.aggregate().unwrap();
        assert_eq!(aggregates.len(), 1);
        let aggregate = &aggregates[0];
        assert_eq!(aggregate.ranking_policy, DEFAULT_RETRIEVAL_POLICY);
        assert_eq!(aggregate.total, 4);
        assert_eq!(aggregate.used, 1);
        assert_eq!(aggregate.helpful, 1);
        assert_eq!(aggregate.ignored, 1);
        assert_eq!(aggregate.corrected, 1);
        assert_eq!(aggregate.usefulness_rate, 0.5);
        assert_eq!(aggregate.ignore_rate, 0.25);
        assert_eq!(aggregate.correction_rate, 0.25);
    }
}
