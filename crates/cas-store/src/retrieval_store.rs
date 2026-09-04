//! Durable retrieval query identity and explicit outcome feedback.
//!
//! Writers are intentionally observational: this module never mutates
//! entry/rule counters or global search weights. Recall consumers may derive
//! bounded, fail-open adjustments from the durable outcome history.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::StoreError;
use crate::shared_db::ImmediateTx;
use crate::{Result, shared_db};

/// Identifier for the unchanged ranking policy observed by this foundation.
pub const DEFAULT_RETRIEVAL_POLICY: &str = "current-default-v1";

/// Attribution labels for durable retrieval feedback.
///
/// The label is deliberately a small, non-identifying tag rather than an
/// actor name.  In particular, sampled relevance labels must be distinguishable
/// from user feedback without putting a model or account identifier in the
/// database.
pub const RETRIEVAL_ATTRIBUTION_EXPLICIT: &str = "explicit";
pub const RETRIEVAL_ATTRIBUTION_AUTOMATIC: &str = "automatic";
pub const RETRIEVAL_ATTRIBUTION_JUDGE: &str = "judge";

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
        outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful', 'unresolved')
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
            outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful', 'unresolved')
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
    /// No evidence of either use or non-use was observed before the session
    /// ended. This is telemetry absence, not negative retrieval evidence.
    Unresolved,
}

impl RetrievalOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Used => "used",
            Self::Helpful => "helpful",
            Self::Ignored => "ignored",
            Self::Corrected => "corrected",
            Self::Harmful => "harmful",
            Self::Unresolved => "unresolved",
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
            "unresolved" => Ok(Self::Unresolved),
            other => Err(StoreError::Parse(format!(
                "invalid retrieval outcome '{other}'; expected used, helpful, ignored, corrected, harmful, or unresolved"
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
    /// Small provenance tag such as `explicit`, `automatic`, or `judge`.
    pub attribution: String,
    pub created_at: DateTime<Utc>,
}

type StoredOutcomeIdentity = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

/// One injected result offered to an offline relevance judge.
///
/// Raw prompt text is intentionally absent: retrieval queries only persist a
/// salted fingerprint.  A receiving-agent judge can still use the current
/// session context, while a scheduled judge can resolve the fingerprint from
/// its own bounded input or return `None` when no label is available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalSample {
    pub query_id: String,
    pub query_fingerprint: String,
    pub query_family: String,
    pub ranking_policy: String,
    pub result_id: String,
    pub document_type: String,
    pub rank: usize,
}

/// Result of one bounded injected-relevance sampling pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RelevanceSamplingReport {
    /// Number of recent unjudged injected result rows offered to the judge.
    pub sampled: usize,
    /// Number of `helpful`/`ignored` rows successfully written.
    pub labels_recorded: usize,
    /// Number of rows for which the judge returned no label.
    /// These rows receive an unresolved marker so they do not consume another
    /// sample until the configured cool-down expires.
    pub unlabeled: usize,
    /// Judge failures are retained as bounded diagnostics; one bad item does
    /// not prevent the remainder of the sample from being evaluated.
    pub judge_errors: Vec<String>,
}

/// Rolling precision for judge-labelled injected results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollingInjectedPrecision {
    pub helpful: u64,
    /// Number of explicit judge labels (`helpful` + `ignored`), i.e. the
    /// denominator for the precision value.
    pub judged: u64,
    pub precision: Option<f64>,
    pub denominator: String,
    pub window_days: u64,
}

/// Evidence stages for one retrieval scope.
///
/// These counts deliberately do not collapse body access, explicit use, or a
/// judge's helpfulness label into one another. A body pull is observable
/// opening evidence; it is not proof that the caller used the content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RetrievalEvidenceFunnel {
    /// Distinct query/result rows returned by retrieval.
    pub retrieved: u64,
    /// Retrieved rows placed in SessionStart/ambient context packets.
    pub injected: u64,
    /// Distinct rows whose body was pulled by an automatic hook signal.
    pub opened: u64,
    /// Distinct rows explicitly marked `used` by a caller.
    pub used: u64,
    /// Distinct rows labelled helpful by the relevance judge.
    pub judged_helpful: u64,
}

/// Offline quality aggregate. Usefulness and negative rates use only resolved
/// outcomes as their denominator; unresolved events report instrumentation
/// coverage without influencing quality or promotion decisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalAggregate {
    pub document_type: String,
    pub query_family: String,
    pub ranking_policy: String,
    pub total: u64,
    /// Number of retrieved result rows represented by this group.
    pub results: u64,
    /// Distinct retrieved result rows with at least one resolved outcome.
    pub resolved_results: u64,
    /// Distinct retrieved result rows with a `used` (body-pull) outcome.
    pub used_results: u64,
    /// Distinct retrieved result rows with a `helpful` outcome.
    pub helpful_results: u64,
    pub resolved: u64,
    pub unresolved: u64,
    /// Number of distinct privacy-preserving sessions contributing resolved
    /// outcomes. Unresolved-only sessions cannot satisfy promotion diversity.
    pub distinct_sessions: u64,
    pub used: u64,
    pub helpful: u64,
    pub ignored: u64,
    pub corrected: u64,
    pub harmful: u64,
    pub usefulness_rate: f64,
    pub ignore_rate: f64,
    pub correction_rate: f64,
    /// Name of the denominator used for all aggregate rates.
    pub denominator: String,
    /// Distinct resolved result rows divided by all retrieved result rows.
    pub coverage_rate: f64,
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

    fn ensure_attribution_column(conn: &Connection) -> Result<()> {
        let has_column: bool = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM pragma_table_info('retrieval_outcomes')
                 WHERE name = 'attribution'
             )",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        if !has_column {
            conn.execute(
                "ALTER TABLE retrieval_outcomes
                 ADD COLUMN attribution TEXT NOT NULL DEFAULT 'explicit'",
                [],
            )?;
        }
        Ok(())
    }

    /// Repair the pre-m244 outcome constraint for direct store users.
    ///
    /// Hook subprocesses open this store without running the numbered CLI
    /// migrations. Rebuild the table in place when its CHECK constraint does
    /// not yet admit `unresolved`; m244's detect query then recognizes the
    /// repaired shape and remains the authoritative migration receipt.
    fn ensure_unresolved_outcome_schema(conn: &Connection) -> Result<()> {
        let supports_unresolved: bool = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'retrieval_outcomes'
                   AND sql LIKE '%''unresolved''%'
             )",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        if supports_unresolved {
            return Ok(());
        }

        let tx = ImmediateTx::new(conn)?;
        Self::ensure_attribution_column(&tx)?;
        tx.execute(
            "CREATE TABLE retrieval_outcomes_new (
                id TEXT PRIMARY KEY,
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                outcome TEXT NOT NULL CHECK (
                    outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful', 'unresolved')
                ),
                actor_hash TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                correction_ref TEXT,
                created_at TEXT NOT NULL,
                attribution TEXT NOT NULL DEFAULT 'explicit',
                FOREIGN KEY (query_id, result_id)
                    REFERENCES retrieval_query_results(query_id, result_id) ON DELETE CASCADE
             )",
            [],
        )?;
        tx.execute(
            "INSERT INTO retrieval_outcomes_new
                (id, query_id, result_id, outcome, actor_hash, session_hash,
                 correction_ref, created_at, attribution)
             SELECT id, query_id, result_id, outcome, actor_hash, session_hash,
                    correction_ref, created_at, attribution
             FROM retrieval_outcomes",
            [],
        )?;
        tx.execute("DROP TABLE retrieval_outcomes", [])?;
        tx.execute(
            "ALTER TABLE retrieval_outcomes_new RENAME TO retrieval_outcomes",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_query
             ON retrieval_outcomes(query_id, result_id)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_created
             ON retrieval_outcomes(created_at)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn parse_aggregate(row: &Row<'_>) -> rusqlite::Result<RetrievalAggregate> {
        let total = row.get::<_, i64>(3)?.max(0) as u64;
        let distinct_sessions = row.get::<_, i64>(4)?.max(0) as u64;
        let used = row.get::<_, i64>(5)?.max(0) as u64;
        let helpful = row.get::<_, i64>(6)?.max(0) as u64;
        let ignored = row.get::<_, i64>(7)?.max(0) as u64;
        let corrected = row.get::<_, i64>(8)?.max(0) as u64;
        let harmful = row.get::<_, i64>(9)?.max(0) as u64;
        let resolved = row.get::<_, i64>(10)?.max(0) as u64;
        let unresolved = row.get::<_, i64>(11)?.max(0) as u64;
        let resolved_results = row.get::<_, i64>(12)?.max(0) as u64;
        let used_results = row.get::<_, i64>(13)?.max(0) as u64;
        let helpful_results = row.get::<_, i64>(14)?.max(0) as u64;
        let results = row.get::<_, i64>(15)?.max(0) as u64;
        let denominator = resolved.max(1) as f64;
        Ok(RetrievalAggregate {
            document_type: row.get(0)?,
            query_family: row.get(1)?,
            ranking_policy: row.get(2)?,
            total,
            results,
            resolved_results,
            used_results,
            helpful_results,
            resolved,
            unresolved,
            distinct_sessions,
            used,
            helpful,
            ignored,
            corrected,
            harmful,
            usefulness_rate: (used + helpful) as f64 / denominator,
            ignore_rate: ignored as f64 / denominator,
            correction_rate: corrected as f64 / denominator,
            denominator: "resolved".to_string(),
            coverage_rate: if results == 0 {
                0.0
            } else {
                resolved_results as f64 / results as f64
            },
        })
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

    /// Aggregate outcomes for one result without changing the observational
    /// retrieval store contract. Consumers use this scoped read to avoid
    /// treating another rule's outcomes as evidence for the current rule.
    pub fn aggregate_for_result(&self, result_id: &str) -> Result<Vec<RetrievalAggregate>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT r.document_type, q.query_family, q.ranking_policy,
                    COUNT(o.id) AS total,
                    COUNT(DISTINCT CASE WHEN o.outcome != 'unresolved' THEN o.session_hash END)
                        AS distinct_sessions,
                    COALESCE(SUM(CASE WHEN o.outcome = 'used' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'helpful' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'ignored' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'corrected' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'harmful' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome != 'unresolved' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'unresolved' THEN 1 ELSE 0 END), 0),
                    COUNT(DISTINCT CASE WHEN o.outcome != 'unresolved'
                        THEN r.query_id || char(0) || r.result_id END) AS resolved_results,
                    COUNT(DISTINCT CASE WHEN o.outcome = 'used'
                        THEN r.query_id || char(0) || r.result_id END) AS used_results,
                    COUNT(DISTINCT CASE WHEN o.outcome = 'helpful'
                        THEN r.query_id || char(0) || r.result_id END) AS helpful_results,
                    COUNT(DISTINCT r.query_id || char(0) || r.result_id) AS results
             FROM retrieval_query_results r
             JOIN retrieval_queries q ON q.id = r.query_id
             LEFT JOIN retrieval_outcomes o
               ON o.query_id = r.query_id AND o.result_id = r.result_id
             WHERE r.result_id = ?1
             GROUP BY r.document_type, q.query_family, q.ranking_policy
             ORDER BY r.document_type, q.query_family, q.ranking_policy",
        )?;
        let rows = stmt.query_map([result_id], Self::parse_aggregate)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    /// Aggregate retrieval rows belonging to one session. Session IDs are
    /// hashed with the same domain-separated identity function used by query
    /// writers; raw session identifiers never enter the SQL query.
    pub fn aggregate_for_session(&self, session_id: &str) -> Result<Vec<RetrievalAggregate>> {
        let session_hash = Self::identity_hash("session", session_id)?;
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT r.document_type, q.query_family, q.ranking_policy,
                    COUNT(o.id) AS total,
                    COUNT(DISTINCT CASE WHEN o.outcome != 'unresolved' THEN o.session_hash END)
                        AS distinct_sessions,
                    COALESCE(SUM(CASE WHEN o.outcome = 'used' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'helpful' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'ignored' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'corrected' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'harmful' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome != 'unresolved' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'unresolved' THEN 1 ELSE 0 END), 0),
                    COUNT(DISTINCT CASE WHEN o.outcome != 'unresolved'
                        THEN r.query_id || char(0) || r.result_id END) AS resolved_results,
                    COUNT(DISTINCT CASE WHEN o.outcome = 'used'
                        THEN r.query_id || char(0) || r.result_id END) AS used_results,
                    COUNT(DISTINCT CASE WHEN o.outcome = 'helpful'
                        THEN r.query_id || char(0) || r.result_id END) AS helpful_results,
                    COUNT(DISTINCT r.query_id || char(0) || r.result_id) AS results
             FROM retrieval_query_results r
             JOIN retrieval_queries q ON q.id = r.query_id
             LEFT JOIN retrieval_outcomes o
               ON o.query_id = r.query_id AND o.result_id = r.result_id
             WHERE q.session_hash = ?1
             GROUP BY r.document_type, q.query_family, q.ranking_policy
             ORDER BY r.document_type, q.query_family, q.ranking_policy",
        )?;
        let rows = stmt.query_map([session_hash], Self::parse_aggregate)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    /// Return stage-separated evidence across all retrieval sessions.
    pub fn evidence_funnel(&self) -> Result<RetrievalEvidenceFunnel> {
        self.evidence_funnel_for_session_hash(None)
    }

    /// Return stage-separated evidence for exactly one raw session ID.
    pub fn evidence_funnel_for_session(&self, session_id: &str) -> Result<RetrievalEvidenceFunnel> {
        let session_hash = Self::identity_hash("session", session_id)?;
        self.evidence_funnel_for_session_hash(Some(session_hash))
    }

    fn evidence_funnel_for_session_hash(
        &self,
        session_hash: Option<String>,
    ) -> Result<RetrievalEvidenceFunnel> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let row = conn.query_row(
            "SELECT
                 COUNT(DISTINCT r.query_id || char(0) || r.result_id),
                 COUNT(DISTINCT CASE
                     WHEN q.query_family = 'context_session_start'
                          OR q.query_family LIKE 'ambient_%'
                     THEN r.query_id || char(0) || r.result_id END),
                 COUNT(DISTINCT CASE
                     WHEN o.outcome = 'used' AND o.attribution = 'automatic'
                     THEN r.query_id || char(0) || r.result_id END),
                 COUNT(DISTINCT CASE
                     WHEN o.outcome = 'used' AND o.attribution = 'explicit'
                     THEN r.query_id || char(0) || r.result_id END),
                 COUNT(DISTINCT CASE
                     WHEN o.outcome = 'helpful' AND o.attribution = 'judge'
                     THEN r.query_id || char(0) || r.result_id END)
             FROM retrieval_query_results r
             JOIN retrieval_queries q ON q.id = r.query_id
             LEFT JOIN retrieval_outcomes o
               ON o.query_id = r.query_id AND o.result_id = r.result_id
             WHERE (?1 IS NULL OR q.session_hash = ?1)",
            [session_hash],
            |row| {
                Ok(RetrievalEvidenceFunnel {
                    retrieved: row.get::<_, i64>(0)?.max(0) as u64,
                    injected: row.get::<_, i64>(1)?.max(0) as u64,
                    opened: row.get::<_, i64>(2)?.max(0) as u64,
                    used: row.get::<_, i64>(3)?.max(0) as u64,
                    judged_helpful: row.get::<_, i64>(4)?.max(0) as u64,
                })
            },
        )?;
        Ok(row)
    }
}

impl RetrievalStore for SqliteRetrievalStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute_batch(RETRIEVAL_SCHEMA)?;
        // Direct store users (including older one-shot tools) may open a
        // database without running the numbered migration runner first.
        // Keep that path compatible while m244/m245 remain the durable
        // upgrades for normal startup.
        Self::ensure_unresolved_outcome_schema(&conn)?;
        Self::ensure_attribution_column(&conn)?;
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
        self.record_outcome_with_attribution(
            id,
            query_id,
            result_id,
            outcome,
            actor_id,
            session_id,
            correction_ref,
            RETRIEVAL_ATTRIBUTION_EXPLICIT,
        )
    }

    fn aggregate(&self) -> Result<Vec<RetrievalAggregate>> {
        SqliteRetrievalStore::aggregate(self)
    }
}

impl SqliteRetrievalStore {
    /// Record an outcome with an explicit provenance tag.
    ///
    /// Existing callers use [`RetrievalStore::record_outcome`] and are marked
    /// `explicit`. Daemon inference uses `automatic`; the relevance sampler
    /// uses `judge`. Keeping the extension on the concrete SQLite store avoids
    /// widening the public trait method and breaking third-party stores.
    pub fn record_outcome_with_attribution(
        &self,
        id: &str,
        query_id: &str,
        result_id: &str,
        outcome: RetrievalOutcome,
        actor_id: &str,
        session_id: &str,
        correction_ref: Option<&str>,
        attribution: &str,
    ) -> Result<RetrievalOutcomeEvent> {
        if let Some(reference) = correction_ref {
            Self::validate_opaque_reference(reference)?;
        }
        Self::validate_opaque_reference(attribution)?;
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
            attribution: attribution.to_string(),
            created_at: Utc::now(),
        };

        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let load_existing = || -> Result<Option<StoredOutcomeIdentity>> {
            conn.query_row(
                "SELECT query_id, result_id, outcome, actor_hash, session_hash,
                        correction_ref, attribution
                 FROM retrieval_outcomes WHERE id = ?1",
                [&event.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(StoreError::Database)
        };
        let is_same_event = |existing: &StoredOutcomeIdentity| {
            existing.0 == event.query_id
                && existing.1 == event.result_id
                && existing.2 == event.outcome.as_str()
                && existing.3 == event.actor_hash
                && existing.4 == event.session_hash
                && existing.5 == event.correction_ref
                && existing.6 == event.attribution
        };
        if let Some(existing) = load_existing()? {
            return if is_same_event(&existing) {
                Ok(event)
            } else {
                Err(StoreError::Other(format!(
                    "retrieval outcome id {id} already records a different event"
                )))
            };
        }

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

        // Automatic hook capture uses a deterministic event ID so a retry is
        // idempotent. Explicit MCP events remain unique UUIDs.
        let inserted = conn.execute(
            "INSERT INTO retrieval_outcomes
             (id, query_id, result_id, outcome, actor_hash, session_hash, correction_ref, attribution, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO NOTHING",
            params![
                event.id,
                event.query_id,
                event.result_id,
                event.outcome.as_str(),
                event.actor_hash,
                event.session_hash,
                event.correction_ref,
                event.attribution,
                event.created_at.to_rfc3339(),
            ],
        )?;
        if inserted == 0 {
            return match load_existing()? {
                Some(existing) if is_same_event(&existing) => Ok(event),
                Some(_) => Err(StoreError::Other(format!(
                    "retrieval outcome id {id} concurrently recorded a different event"
                ))),
                None => Err(StoreError::Other(format!(
                    "retrieval outcome {id} was not persisted"
                ))),
            };
        }
        Ok(event)
    }

    /// Offer the newest unjudged injected result rows to a relevance judge.
    ///
    /// The callback returns `Some(true)` for a relevant result,
    /// `Some(false)` for an irrelevant result, and `None` when the receiving
    /// agent or scheduled judge cannot label the item. Unlabelled attempts are
    /// marked unresolved and excluded for `cooldown_secs`; resolved labels are
    /// excluded permanently. A judge failure is isolated to that item; storage
    /// failures still fail the pass because a partial write cannot be reported
    /// as a successful label.
    pub fn sample_injected_relevance<F>(
        &self,
        sample_size: usize,
        cooldown_secs: u64,
        mut judge: F,
    ) -> Result<RelevanceSamplingReport>
    where
        F: FnMut(&RetrievalSample) -> std::result::Result<Option<bool>, String>,
    {
        if sample_size == 0 {
            return Ok(RelevanceSamplingReport::default());
        }

        let cutoff = i64::try_from(cooldown_secs)
            .ok()
            .and_then(chrono::Duration::try_seconds)
            .and_then(|duration| Utc::now().checked_sub_signed(duration))
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
            .to_rfc3339();
        let samples = {
            let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            let mut stmt = conn.prepare(
                "SELECT r.query_id, q.query_fingerprint, q.query_family,
                        q.ranking_policy, r.result_id, r.document_type, r.rank
                 FROM retrieval_query_results r
                 JOIN retrieval_queries q ON q.id = r.query_id
                 WHERE (q.query_family = 'context_session_start'
                        OR q.query_family LIKE 'ambient_%')
                   AND NOT EXISTS (
                       SELECT 1 FROM retrieval_outcomes judged
                       WHERE judged.query_id = r.query_id
                         AND judged.result_id = r.result_id
                         AND judged.attribution = 'judge'
                         AND (judged.outcome != 'unresolved'
                              OR judged.created_at >= ?2)
                   )
                 ORDER BY q.created_at DESC, r.rank ASC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![sample_size as i64, cutoff], |row| {
                Ok(RetrievalSample {
                    query_id: row.get(0)?,
                    query_fingerprint: row.get(1)?,
                    query_family: row.get(2)?,
                    ranking_policy: row.get(3)?,
                    result_id: row.get(4)?,
                    document_type: row.get(5)?,
                    rank: row.get::<_, i64>(6)?.max(0) as usize,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut report = RelevanceSamplingReport {
            sampled: samples.len(),
            ..Default::default()
        };
        for sample in samples {
            let label = match judge(&sample) {
                Ok(label) => label,
                Err(error) => {
                    report.judge_errors.push(format!(
                        "{}:{}: {error}",
                        sample.query_id, sample.result_id
                    ));
                    None
                }
            };
            let Some(label) = label else {
                report.unlabeled += 1;
                let event_id = format!(
                    "out-judge-unresolved-{}",
                    Self::fingerprint(
                        "judge-unresolved-event",
                        &format!(
                            "{}\0{}\0{}",
                            sample.query_id,
                            sample.result_id,
                            Utc::now().timestamp_nanos_opt().unwrap_or_default()
                        ),
                    )
                    .trim_start_matches("sha256:")
                );
                let session_id = format!("judge:{}", sample.query_id);
                self.record_outcome_with_attribution(
                    &event_id,
                    &sample.query_id,
                    &sample.result_id,
                    RetrievalOutcome::Unresolved,
                    "retrieval-relevance-judge",
                    &session_id,
                    None,
                    RETRIEVAL_ATTRIBUTION_JUDGE,
                )?;
                continue;
            };
            let outcome = if label {
                RetrievalOutcome::Helpful
            } else {
                RetrievalOutcome::Ignored
            };
            let event_id = format!(
                "out-judge-{}",
                Self::fingerprint(
                    "judge-event",
                    &format!("{}\0{}", sample.query_id, sample.result_id),
                )
                .trim_start_matches("sha256:")
            );
            let session_id = format!("judge:{}", sample.query_id);
            self.record_outcome_with_attribution(
                &event_id,
                &sample.query_id,
                &sample.result_id,
                outcome,
                "retrieval-relevance-judge",
                &session_id,
                None,
                RETRIEVAL_ATTRIBUTION_JUDGE,
            )?;
            report.labels_recorded += 1;
        }
        Ok(report)
    }

    /// Compute precision over judge-labelled injected results in a rolling
    /// window. A missing denominator is `None`, not a fabricated zero.
    pub fn rolling_injected_precision(&self, window_days: u64) -> Result<RollingInjectedPrecision> {
        self.rolling_injected_precision_for_session_hash(window_days, None)
    }

    /// Compute rolling judge precision for exactly one retrieval session.
    pub fn rolling_injected_precision_for_session(
        &self,
        window_days: u64,
        session_id: &str,
    ) -> Result<RollingInjectedPrecision> {
        let session_hash = Self::identity_hash("session", session_id)?;
        self.rolling_injected_precision_for_session_hash(window_days, Some(session_hash))
    }

    fn rolling_injected_precision_for_session_hash(
        &self,
        window_days: u64,
        session_hash: Option<String>,
    ) -> Result<RollingInjectedPrecision> {
        let cutoff = Utc::now() - chrono::Duration::days(window_days as i64);
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let (helpful, judged): (i64, i64) = conn.query_row(
            "SELECT
                 COALESCE(SUM(CASE WHEN o.outcome = 'helpful' THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN o.outcome IN ('helpful', 'ignored') THEN 1 ELSE 0 END), 0)
             FROM retrieval_outcomes o
             JOIN retrieval_queries q ON q.id = o.query_id
             WHERE o.attribution = 'judge'
               AND o.created_at >= ?1
               AND (q.query_family = 'context_session_start'
                    OR q.query_family LIKE 'ambient_%')
               AND (?2 IS NULL OR q.session_hash = ?2)",
            params![cutoff.to_rfc3339(), session_hash],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let helpful = helpful.max(0) as u64;
        let judged = judged.max(0) as u64;
        Ok(RollingInjectedPrecision {
            helpful,
            judged,
            precision: (judged > 0).then(|| helpful as f64 / judged as f64),
            denominator: "judge_labels".to_string(),
            window_days,
        })
    }

    /// Aggregate retrieval outcomes by document type, query family, and
    /// ranking policy. Result-stage counts are distinct rows so repeated
    /// outcome events cannot inflate the funnel or its coverage denominator.
    pub fn aggregate(&self) -> Result<Vec<RetrievalAggregate>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT r.document_type, q.query_family, q.ranking_policy,
                    COUNT(o.id) AS total,
                    COUNT(DISTINCT CASE WHEN o.outcome != 'unresolved' THEN o.session_hash END)
                        AS distinct_sessions,
                    COALESCE(SUM(CASE WHEN o.outcome = 'used' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'helpful' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'ignored' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'corrected' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'harmful' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome != 'unresolved' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN o.outcome = 'unresolved' THEN 1 ELSE 0 END), 0),
                    COUNT(DISTINCT CASE WHEN o.outcome != 'unresolved'
                        THEN r.query_id || char(0) || r.result_id END) AS resolved_results,
                    COUNT(DISTINCT CASE WHEN o.outcome = 'used'
                        THEN r.query_id || char(0) || r.result_id END) AS used_results,
                    COUNT(DISTINCT CASE WHEN o.outcome = 'helpful'
                        THEN r.query_id || char(0) || r.result_id END) AS helpful_results,
                    COUNT(DISTINCT r.query_id || char(0) || r.result_id) AS results
             FROM retrieval_query_results r
             JOIN retrieval_queries q ON q.id = r.query_id
             LEFT JOIN retrieval_outcomes o
               ON o.query_id = r.query_id AND o.result_id = r.result_id
             GROUP BY r.document_type, q.query_family, q.ranking_policy
             ORDER BY r.document_type, q.query_family, q.ranking_policy",
        )?;
        let rows = stmt.query_map([], Self::parse_aggregate)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn hit(id: &str, document_type: &str, rank: usize) -> RetrievalHitIdentity {
        RetrievalHitIdentity {
            result_id: id.to_string(),
            document_type: document_type.to_string(),
            rank,
        }
    }

    #[test]
    fn unresolved_is_a_supported_retrieval_outcome() {
        assert!(RetrievalOutcome::from_str("unresolved").is_ok());
    }

    #[test]
    fn opening_pre_m244_schema_never_silently_drops_unresolved_outcome() {
        let temp = TempDir::new().unwrap();
        let conn = Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE retrieval_queries (
                id TEXT PRIMARY KEY,
                query_fingerprint TEXT NOT NULL,
                query_family TEXT NOT NULL,
                ranking_policy TEXT NOT NULL,
                session_hash TEXT,
                created_at TEXT NOT NULL
             );
             CREATE TABLE retrieval_query_results (
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                document_type TEXT NOT NULL,
                rank INTEGER NOT NULL,
                PRIMARY KEY (query_id, result_id),
                FOREIGN KEY (query_id) REFERENCES retrieval_queries(id) ON DELETE CASCADE
             );
             CREATE TABLE retrieval_outcomes (
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
             INSERT INTO retrieval_queries VALUES
                ('q1', 'fingerprint', 'ambient_transition', 'policy', 'session', 'now');
             INSERT INTO retrieval_query_results VALUES ('q1', 'entry-1', 'entry', 0);",
        )
        .unwrap();
        drop(conn);

        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_outcome_with_attribution(
                "out-unresolved",
                "q1",
                "entry-1",
                RetrievalOutcome::Unresolved,
                "actor",
                "session",
                None,
                RETRIEVAL_ATTRIBUTION_AUTOMATIC,
            )
            .unwrap();

        let conn = store.conn.lock().unwrap();
        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM retrieval_outcomes
                 WHERE id = 'out-unresolved' AND outcome = 'unresolved'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted, 1, "a successful outcome write must be durable");
        let attribution: String = conn
            .query_row(
                "SELECT attribution FROM retrieval_outcomes WHERE id = 'out-unresolved'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attribution, RETRIEVAL_ATTRIBUTION_AUTOMATIC);
        let m244_detected: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'retrieval_outcomes'
                   AND sql LIKE '%''unresolved''%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(m244_detected, 1, "the ordered migration must detect the inline repair");
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
        assert_eq!(event.attribution, RETRIEVAL_ATTRIBUTION_EXPLICIT);

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
            ("out-5", "entry-2", RetrievalOutcome::Unresolved, None),
        ] {
            store
                .record_outcome(id, "qry-1", result, outcome, "actor", "session", correction)
                .unwrap();
        }
        // Hook retries reuse a deterministic event ID. Replaying the same
        // identity must be idempotent rather than double-counting or failing.
        store
            .record_outcome(
                "out-1",
                "qry-1",
                "entry-1",
                RetrievalOutcome::Used,
                "actor",
                "session",
                None,
            )
            .unwrap();
        assert!(
            store
                .record_outcome(
                    "out-1",
                    "qry-1",
                    "entry-1",
                    RetrievalOutcome::Harmful,
                    "actor",
                    "session",
                    None,
                )
                .is_err(),
            "reusing an event ID for different evidence is not an idempotent retry"
        );

        let aggregates = store.aggregate().unwrap();
        assert_eq!(aggregates.len(), 1);
        let aggregate = &aggregates[0];
        assert_eq!(aggregate.ranking_policy, DEFAULT_RETRIEVAL_POLICY);
        assert_eq!(aggregate.total, 5);
        assert_eq!(aggregate.results, 2);
        assert_eq!(aggregate.resolved_results, 2);
        assert_eq!(aggregate.used_results, 1);
        assert_eq!(aggregate.helpful_results, 1);
        assert_eq!(aggregate.resolved, 4);
        assert_eq!(aggregate.unresolved, 1);
        assert_eq!(aggregate.denominator, "resolved");
        assert_eq!(aggregate.coverage_rate, 1.0);
        assert_eq!(aggregate.distinct_sessions, 1);
        assert_eq!(aggregate.used, 1);
        assert_eq!(aggregate.helpful, 1);
        assert_eq!(aggregate.ignored, 1);
        assert_eq!(aggregate.corrected, 1);
        assert_eq!(aggregate.usefulness_rate, 0.5);
        assert_eq!(aggregate.ignore_rate, 0.25);
        assert_eq!(aggregate.correction_rate, 0.25);
    }

    #[test]
    fn aggregates_count_distinct_privacy_preserving_sessions() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        for (query_id, outcome_id, session_id) in [
            ("qry-session-1", "out-session-1", "session-one"),
            ("qry-session-2", "out-session-2", "session-two"),
        ] {
            store
                .record_query(
                    query_id,
                    "query",
                    "keyword",
                    DEFAULT_RETRIEVAL_POLICY,
                    Some(session_id),
                    &[hit("rule-1", "rule", 0)],
                )
                .unwrap();
            store
                .record_outcome(
                    outcome_id,
                    query_id,
                    "rule-1",
                    RetrievalOutcome::Helpful,
                    "actor",
                    session_id,
                    None,
                )
                .unwrap();
        }

        let aggregates = store.aggregate_for_result("rule-1").unwrap();
        assert_eq!(aggregates.len(), 1);
        assert_eq!(aggregates[0].distinct_sessions, 2);
        assert_eq!(aggregates[0].helpful, 2);
    }

    #[test]
    fn aggregates_results_without_outcomes_with_zero_coverage() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_query(
                "qry-unresolved-only",
                "query with no feedback",
                "context_session_start",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[hit("entry-unresolved-only", "entry", 0)],
            )
            .unwrap();

        let aggregate = &store.aggregate().unwrap()[0];
        assert_eq!(aggregate.results, 1);
        assert_eq!(aggregate.resolved_results, 0);
        assert_eq!(aggregate.used_results, 0);
        assert_eq!(aggregate.helpful_results, 0);
        assert_eq!(aggregate.coverage_rate, 0.0);
    }

    #[test]
    fn samples_recent_injected_results_once_with_judge_attribution() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_query(
                "ambient-query",
                "repair parser cache",
                "ambient_transition",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[hit("entry-helpful", "entry", 0), hit("entry-ignored", "entry", 1)],
            )
            .unwrap();
        store
            .record_query(
                "ordinary-query",
                "not an injected packet",
                "keyword",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[hit("ordinary", "entry", 0)],
            )
            .unwrap();

        let report = store
            .sample_injected_relevance(10, 604_800, |sample| {
                Ok(Some(sample.result_id == "entry-helpful"))
            })
            .unwrap();
        assert_eq!(report.sampled, 2);
        assert_eq!(report.labels_recorded, 2);
        assert_eq!(report.unlabeled, 0);
        assert!(report.judge_errors.is_empty());

        let precision = store.rolling_injected_precision(30).unwrap();
        assert_eq!(precision.helpful, 1);
        assert_eq!(precision.judged, 2);
        assert_eq!(precision.precision, Some(0.5));

        let conn = store.conn.lock().unwrap();
        let judge_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM retrieval_outcomes WHERE attribution = 'judge'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(judge_rows, 2);
        drop(conn);

        let second = store
            .sample_injected_relevance(10, 604_800, |_| Ok(Some(true)))
            .unwrap();
        assert_eq!(second.sampled, 0, "judge rows are idempotently excluded");
    }

    #[test]
    fn session_metrics_keep_open_use_and_judge_evidence_separate() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        for (query_id, session_id) in [("query-a", "session-a"), ("query-b", "session-b")] {
            store
                .record_query(
                    query_id,
                    "injected context",
                    "ambient_transition",
                    DEFAULT_RETRIEVAL_POLICY,
                    Some(session_id),
                    &[hit("entry", "entry", 0)],
                )
                .unwrap();
        }
        store
            .record_outcome_with_attribution(
                "opened-a",
                "query-a",
                "entry",
                RetrievalOutcome::Used,
                "hook",
                "session-a",
                None,
                RETRIEVAL_ATTRIBUTION_AUTOMATIC,
            )
            .unwrap();
        store
            .record_outcome(
                "used-a",
                "query-a",
                "entry",
                RetrievalOutcome::Used,
                "agent",
                "session-a",
                None,
            )
            .unwrap();
        for (event_id, query_id, session_id, outcome) in [
            ("judge-a", "query-a", "session-a", RetrievalOutcome::Ignored),
            ("judge-b", "query-b", "session-b", RetrievalOutcome::Helpful),
        ] {
            store
                .record_outcome_with_attribution(
                    event_id,
                    query_id,
                    "entry",
                    outcome,
                    "judge",
                    session_id,
                    None,
                    RETRIEVAL_ATTRIBUTION_JUDGE,
                )
                .unwrap();
        }
        for (event_id, attribution) in [
            ("judge-used-b", RETRIEVAL_ATTRIBUTION_JUDGE),
            ("custom-used-b", "future-signal"),
        ] {
            store
                .record_outcome_with_attribution(
                    event_id,
                    "query-b",
                    "entry",
                    RetrievalOutcome::Used,
                    "non-caller",
                    "session-b",
                    None,
                    attribution,
                )
                .unwrap();
        }

        let session_a = store.evidence_funnel_for_session("session-a").unwrap();
        assert_eq!(session_a.retrieved, 1);
        assert_eq!(session_a.injected, 1);
        assert_eq!(session_a.opened, 1);
        assert_eq!(session_a.used, 1);
        assert_eq!(session_a.judged_helpful, 0);

        let session_b = store.evidence_funnel_for_session("session-b").unwrap();
        assert_eq!(session_b.retrieved, 1);
        assert_eq!(session_b.injected, 1);
        assert_eq!(session_b.opened, 0);
        assert_eq!(session_b.used, 0);
        assert_eq!(session_b.judged_helpful, 1);

        let precision_a = store
            .rolling_injected_precision_for_session(30, "session-a")
            .unwrap();
        let precision_b = store
            .rolling_injected_precision_for_session(30, "session-b")
            .unwrap();
        assert_eq!(precision_a.precision, Some(0.0));
        assert_eq!(precision_a.judged, 1);
        assert_eq!(precision_b.precision, Some(1.0));
        assert_eq!(precision_b.judged, 1);
        assert_eq!(
            store.rolling_injected_precision(30).unwrap().precision,
            Some(0.5)
        );
    }

    #[test]
    fn unlabeled_injected_results_enter_a_cooldown() {
        let temp = TempDir::new().unwrap();
        let store = SqliteRetrievalStore::open(temp.path()).unwrap();
        store
            .record_query(
                "ambient-unlabeled",
                "query unavailable to judge",
                "ambient_transition",
                DEFAULT_RETRIEVAL_POLICY,
                Some("session"),
                &[hit("entry-unlabeled", "entry", 0)],
            )
            .unwrap();

        let first = store
            .sample_injected_relevance(1, 3_600, |_| Ok(None))
            .unwrap();
        assert_eq!(first.sampled, 1);
        assert_eq!(first.labels_recorded, 0);
        assert_eq!(first.unlabeled, 1);

        let second = store
            .sample_injected_relevance(1, 3_600, |_| Ok(Some(true)))
            .unwrap();
        assert_eq!(
            second.sampled, 0,
            "the same item must not consume the next judge pass"
        );

        let conn = store.conn.lock().unwrap();
        let skips: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM retrieval_outcomes
                 WHERE attribution = 'judge' AND outcome = 'unresolved'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(skips, 1, "the cool-down marker is durable but not a label");
        conn.execute(
            "UPDATE retrieval_outcomes SET created_at = '2000-01-01T00:00:00+00:00'
             WHERE attribution = 'judge' AND outcome = 'unresolved'",
            [],
        )
        .unwrap();
        drop(conn);

        let after_cooldown = store
            .sample_injected_relevance(1, 3_600, |_| Ok(Some(true)))
            .unwrap();
        assert_eq!(after_cooldown.sampled, 1);
        assert_eq!(after_cooldown.labels_recorded, 1);
        assert_eq!(after_cooldown.unlabeled, 0);
        let precision = store.rolling_injected_precision(30).unwrap();
        assert_eq!(precision.helpful, 1);
        assert_eq!(precision.judged, 1, "unresolved skips are not judge labels");
    }
}
