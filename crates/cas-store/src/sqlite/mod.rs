//! SQLite storage backend

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use std::str::FromStr;

use crate::Result;
use cas_types::{
    BeliefType, Entry, EntryType, MemoryTier, ObservationType, Rule, RuleStatus, Scope,
};

/// SQLite DDL for the `entries`, `rules`, and `metadata` tables and their
/// indexes — the core memory + rules schema.
///
/// Re-exported via `cas_store::ENTRIES_RULES_SCHEMA` so the migration runner
/// in `cas-cli` can bootstrap the base tables before applying ALTER migrations
/// against subsystems whose tables were historically created lazily by
/// `SqliteStore::init` / `SqliteRuleStore::init`. See cas-bdb9 / EPIC cas-9fdb.
pub const ENTRIES_RULES_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS entries (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL DEFAULT 'learning',
    tags TEXT,
    created TEXT NOT NULL,
    helpful_count INTEGER NOT NULL DEFAULT 0,
    harmful_count INTEGER NOT NULL DEFAULT 0,
    last_accessed TEXT,
    title TEXT,
    content TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    -- Hook support columns
    session_id TEXT,
    source_tool TEXT,
    -- AI extraction pipeline
    pending_extraction INTEGER NOT NULL DEFAULT 0,
    observation_type TEXT,
    -- Memory decay columns
    stability REAL NOT NULL DEFAULT 0.5,
    access_count INTEGER NOT NULL DEFAULT 0,
    -- Memory compression
    raw_content TEXT,
    compressed INTEGER NOT NULL DEFAULT 0,
    -- Tiered storage
    memory_tier TEXT NOT NULL DEFAULT 'working',
    importance REAL NOT NULL DEFAULT 0.5,
    -- Temporal validity
    valid_from TEXT,
    valid_until TEXT,
    review_after TEXT,
    -- Learning review tracking
    last_reviewed TEXT,
    -- Background embedding
    pending_embedding INTEGER NOT NULL DEFAULT 1,
    -- Hindsight-inspired belief tracking
    belief_type TEXT NOT NULL DEFAULT 'fact',
    confidence REAL NOT NULL DEFAULT 1.0,
    -- Context-aware knowledge
    domain TEXT,
    -- Worktree scoping
    branch TEXT,
    -- Scope (global vs project)
    scope TEXT NOT NULL DEFAULT 'project',
    -- Team collaboration
    team_id TEXT,
    -- Team-promotion share override (private | team)
    share TEXT,
    -- Incremental indexing tracking
    updated_at TEXT,
    indexed_at TEXT,
    -- Entry provenance (JSON array of source entry IDs)
    source_ids TEXT
);

CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created DESC);
CREATE INDEX IF NOT EXISTS idx_entries_session ON entries(session_id);
CREATE INDEX IF NOT EXISTS idx_entries_pending ON entries(pending_extraction);
CREATE INDEX IF NOT EXISTS idx_entries_obs_type ON entries(observation_type);
CREATE INDEX IF NOT EXISTS idx_entries_stability ON entries(stability);
CREATE INDEX IF NOT EXISTS idx_entries_importance ON entries(importance);
CREATE INDEX IF NOT EXISTS idx_entries_pending_embedding ON entries(pending_embedding) WHERE pending_embedding = 1;
CREATE INDEX IF NOT EXISTS idx_entries_confidence ON entries(confidence);
CREATE INDEX IF NOT EXISTS idx_entries_domain ON entries(domain);
CREATE INDEX IF NOT EXISTS idx_entries_branch ON entries(branch);
CREATE INDEX IF NOT EXISTS idx_entries_scope ON entries(scope);
CREATE INDEX IF NOT EXISTS idx_entries_pending_index ON entries(updated_at) WHERE indexed_at IS NULL OR updated_at > indexed_at;
CREATE INDEX IF NOT EXISTS idx_entries_unreviewed_learnings ON entries(created DESC) WHERE type = 'learning' AND archived = 0 AND last_reviewed IS NULL;

CREATE TABLE IF NOT EXISTS rules (
    id TEXT PRIMARY KEY,
    created TEXT NOT NULL,
    source_ids TEXT,
    helpful_count INTEGER NOT NULL DEFAULT 0,
    harmful_count INTEGER NOT NULL DEFAULT 0,
    tags TEXT,
    paths TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft',
    last_accessed TEXT,
    review_after TEXT,
    -- Rule automation
    hook_command TEXT,
    -- CodeRabbit-inspired categorization
    category TEXT NOT NULL DEFAULT 'general',
    priority INTEGER NOT NULL DEFAULT 2,
    -- Surface tracking
    surface_count INTEGER NOT NULL DEFAULT 0,
    -- Scope (global vs project)
    scope TEXT NOT NULL DEFAULT 'project',
    -- PreToolUse auto-approval (comma-separated tool names)
    auto_approve_tools TEXT,
    -- Path patterns for auto-approval (comma-separated globs)
    auto_approve_paths TEXT,
    -- Team collaboration
    team_id TEXT,
    -- Team-promotion share override (private | team)
    share TEXT
);

CREATE INDEX IF NOT EXISTS idx_rules_created ON rules(created DESC);
CREATE INDEX IF NOT EXISTS idx_rules_status ON rules(status);
CREATE INDEX IF NOT EXISTS idx_rules_category ON rules(category);
CREATE INDEX IF NOT EXISTS idx_rules_priority ON rules(priority);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Sessions table for enterprise analytics. Inline-bootstrap so migrations
-- such as m042_sessions_add_outcome / sessions_add_title can ALTER it on
-- DBs that have never had `SqliteStore::store_init` run.
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    cwd TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    duration_secs INTEGER,
    permission_mode TEXT,
    entries_created INTEGER NOT NULL DEFAULT 0,
    tasks_closed INTEGER NOT NULL DEFAULT 0,
    tool_uses INTEGER NOT NULL DEFAULT 0,
    team_id TEXT,
    title TEXT,
    branch TEXT,
    worktree_id TEXT,
    outcome TEXT,
    friction_score REAL,
    delight_count INTEGER DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_team ON sessions(team_id);

-- Expression index for helpful-score sort (requires SQLite 3.31+). Previously
-- created via a best-effort inline `let _ = conn.execute(...)` in
-- `store_init`; lifted here so the migration-runner bootstrap installs it
-- alongside the rest of the entries schema. SQLite 3.31 has been minimum-
-- supported across the supported platform matrix for years, so the
-- silent-skip fallback is no longer required.
CREATE INDEX IF NOT EXISTS idx_entries_helpful_score ON entries(
    (helpful_count - harmful_count) DESC,
    last_accessed DESC
) WHERE archived = 0 AND (helpful_count - harmful_count) > 0;
"#;

/// SQLite-based entry store
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    cas_dir: PathBuf,
}

impl SqliteStore {
    /// Open or create a SQLite store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        Ok(Self {
            conn,
            cas_dir: cas_dir.to_path_buf(),
        })
    }

    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        // Try RFC3339 first
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        // Try common SQLite format
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    fn parse_tags(s: Option<String>) -> Vec<String> {
        s.map(|tags| {
            if tags.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&tags)
                    .unwrap_or_else(|_| tags.split(',').map(|t| t.trim().to_string()).collect())
            }
        })
        .unwrap_or_default()
    }

    fn tags_to_string(tags: &[String]) -> String {
        if tags.is_empty() {
            String::new()
        } else {
            serde_json::to_string(tags).unwrap_or_default()
        }
    }

    fn parse_source_ids(s: Option<String>) -> Vec<String> {
        s.map(|ids| {
            if ids.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&ids)
                    .unwrap_or_else(|_| ids.split(',').map(|id| id.trim().to_string()).collect())
            }
        })
        .unwrap_or_default()
    }

    fn source_ids_to_string(ids: &[String]) -> String {
        if ids.is_empty() {
            String::new()
        } else {
            serde_json::to_string(ids).unwrap_or_default()
        }
    }

    /// Construct an `Entry` from a row selected with the standard 32-column projection.
    ///
    /// Expected column order (indices 0–32):
    ///   id, type, tags, created, content, title, helpful_count,
    ///   harmful_count, last_accessed, archived, session_id, source_tool,
    ///   pending_extraction, observation_type, stability, access_count,
    ///   raw_content, compressed, memory_tier, importance, valid_from, valid_until,
    ///   review_after, last_reviewed, pending_embedding, belief_type, confidence,
    ///   domain, branch, scope, team_id, share, source_ids
    fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<Entry> {
        Ok(Entry {
            id: row.get(0)?,
            entry_type: row
                .get::<_, String>(1)?
                .parse()
                .unwrap_or(EntryType::Learning),
            observation_type: Self::parse_observation_type(row.get(13)?),
            tags: Self::parse_tags(row.get(2)?),
            created: Self::parse_datetime(&row.get::<_, String>(3)?).unwrap_or_else(Utc::now),
            content: row.get(4)?,
            raw_content: row.get(16)?,
            compressed: row.get::<_, i32>(17).unwrap_or(0) != 0,
            memory_tier: Self::parse_memory_tier(row.get(18)?),
            title: row.get(5)?,
            helpful_count: row.get(6)?,
            harmful_count: row.get(7)?,
            last_accessed: row
                .get::<_, Option<String>>(8)?
                .and_then(|s| Self::parse_datetime(&s)),
            archived: row.get::<_, i32>(9)? != 0,
            session_id: row.get(10)?,
            source_tool: row.get(11)?,
            source_ids: Self::parse_source_ids(row.get(32)?),
            pending_extraction: row.get::<_, i32>(12).unwrap_or(0) != 0,
            stability: row.get::<_, f32>(14).unwrap_or(0.5),
            access_count: row.get::<_, i32>(15).unwrap_or(0),
            importance: row.get::<_, f32>(19).unwrap_or(0.5),
            valid_from: row
                .get::<_, Option<String>>(20)?
                .and_then(|s| Self::parse_datetime(&s)),
            valid_until: row
                .get::<_, Option<String>>(21)?
                .and_then(|s| Self::parse_datetime(&s)),
            review_after: row
                .get::<_, Option<String>>(22)?
                .and_then(|s| Self::parse_datetime(&s)),
            last_reviewed: row
                .get::<_, Option<String>>(23)?
                .and_then(|s| Self::parse_datetime(&s)),
            pending_embedding: row.get::<_, i32>(24).unwrap_or(1) != 0,
            belief_type: Self::parse_belief_type(row.get(25)?),
            confidence: row.get::<_, f32>(26).unwrap_or(1.0),
            domain: row.get(27)?,
            branch: row.get(28)?,
            scope: row
                .get::<_, Option<String>>(29)?
                .map(|s| Scope::from_str(&s).unwrap_or_default())
                .unwrap_or_default(),
            team_id: row.get(30)?,
            share: row
                .get::<_, Option<String>>(31)?
                .as_deref()
                .and_then(|s| s.parse().ok()),
        })
    }

    fn parse_observation_type(s: Option<String>) -> Option<ObservationType> {
        s.and_then(|t| t.parse().ok())
    }

    fn parse_memory_tier(s: Option<String>) -> MemoryTier {
        s.and_then(|t| t.parse().ok())
            .unwrap_or(MemoryTier::Working)
    }

    fn parse_belief_type(s: Option<String>) -> BeliefType {
        s.and_then(|t| t.parse().ok()).unwrap_or(BeliefType::Fact)
    }

    /// Start a new session
    pub fn start_session(&self, session: &cas_types::Session) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO sessions (session_id, cwd, started_at, permission_mode, team_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.session_id,
                session.cwd,
                session.started_at.to_rfc3339(),
                session.permission_mode,
                session.team_id,
            ],
        )?;
        Ok(())
    }

    /// End a session and compute duration
    pub fn end_session(&self, session_id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now();

        // Get session start time
        let started_at: Option<String> = conn
            .query_row(
                "SELECT started_at FROM sessions WHERE session_id = ?",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(started_str) = started_at {
            if let Some(started) = Self::parse_datetime(&started_str) {
                let duration_secs = (now - started).num_seconds();
                conn.execute(
                    "UPDATE sessions SET ended_at = ?1, duration_secs = ?2 WHERE session_id = ?3",
                    params![now.to_rfc3339(), duration_secs, session_id],
                )?;
            }
        }
        Ok(())
    }

    /// Update session metrics (entries created, tasks closed, tool uses)
    pub fn update_session_metrics(
        &self,
        session_id: &str,
        entries_delta: i32,
        tasks_delta: i32,
        tool_uses_delta: i32,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE sessions SET
                entries_created = entries_created + ?1,
                tasks_closed = tasks_closed + ?2,
                tool_uses = tool_uses + ?3
             WHERE session_id = ?4",
            params![entries_delta, tasks_delta, tool_uses_delta, session_id],
        )?;
        Ok(())
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> Result<Option<cas_types::Session>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT session_id, cwd, started_at, ended_at, duration_secs,
                    permission_mode, entries_created, tasks_closed, tool_uses, team_id, title,
                    outcome, friction_score, delight_count
             FROM sessions WHERE session_id = ?",
            params![session_id],
            |row| {
                let started_at_str: String = row.get(2)?;
                let ended_at_str: Option<String> = row.get(3)?;
                let outcome_str: Option<String> = row.get(11)?;

                Ok(cas_types::Session {
                    session_id: row.get(0)?,
                    cwd: row.get(1)?,
                    started_at: Self::parse_datetime(&started_at_str).unwrap_or_else(Utc::now),
                    ended_at: ended_at_str.as_ref().and_then(|s| Self::parse_datetime(s)),
                    duration_secs: row.get(4)?,
                    permission_mode: row.get(5)?,
                    entries_created: row.get::<_, i32>(6)? as u32,
                    tasks_closed: row.get::<_, i32>(7)? as u32,
                    tool_uses: row.get::<_, i32>(8)? as u32,
                    team_id: row.get(9)?,
                    title: row.get(10)?,
                    outcome: outcome_str.and_then(|s| s.parse().ok()),
                    friction_score: row.get(12)?,
                    delight_count: row.get::<_, Option<i32>>(13)?.unwrap_or(0) as u32,
                })
            },
        )
        .optional()
        .map_err(|e| e.into())
    }

    /// List recent sessions (for sync)
    pub fn list_sessions_since(&self, since: DateTime<Utc>) -> Result<Vec<cas_types::Session>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT session_id, cwd, started_at, ended_at, duration_secs,
                    permission_mode, entries_created, tasks_closed, tool_uses, team_id, title,
                    outcome, friction_score, delight_count
             FROM sessions WHERE started_at >= ? ORDER BY started_at DESC",
        )?;

        let sessions = stmt.query_map(params![since.to_rfc3339()], |row| {
            let started_at_str: String = row.get(2)?;
            let ended_at_str: Option<String> = row.get(3)?;
            let outcome_str: Option<String> = row.get(11)?;

            Ok(cas_types::Session {
                session_id: row.get(0)?,
                cwd: row.get(1)?,
                started_at: Self::parse_datetime(&started_at_str).unwrap_or_else(Utc::now),
                ended_at: ended_at_str.as_ref().and_then(|s| Self::parse_datetime(s)),
                duration_secs: row.get(4)?,
                permission_mode: row.get(5)?,
                entries_created: row.get::<_, i32>(6)? as u32,
                tasks_closed: row.get::<_, i32>(7)? as u32,
                tool_uses: row.get::<_, i32>(8)? as u32,
                team_id: row.get(9)?,
                title: row.get(10)?,
                outcome: outcome_str.and_then(|s| s.parse().ok()),
                friction_score: row.get(12)?,
                delight_count: row.get::<_, Option<i32>>(13)?.unwrap_or(0) as u32,
            })
        })?;

        sessions
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// Update session title
    pub fn update_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE sessions SET title = ?1 WHERE session_id = ?2",
            params![title, session_id],
        )?;
        Ok(())
    }

    /// Update session outcome
    pub fn update_session_outcome(
        &self,
        session_id: &str,
        outcome: cas_types::SessionOutcome,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE sessions SET outcome = ?1 WHERE session_id = ?2",
            params![outcome.to_string(), session_id],
        )?;
        Ok(())
    }

    /// Update session friction score
    pub fn update_session_friction_score(&self, session_id: &str, score: f32) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let clamped = score.clamp(0.0, 1.0);
        conn.execute(
            "UPDATE sessions SET friction_score = ?1 WHERE session_id = ?2",
            params![clamped, session_id],
        )?;
        Ok(())
    }

    /// Increment session delight count
    pub fn increment_session_delight(&self, session_id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE sessions SET delight_count = COALESCE(delight_count, 0) + 1 WHERE session_id = ?",
            params![session_id],
        )?;
        Ok(())
    }

    /// Update all session signals at once (for batch updates)
    pub fn update_session_signals(
        &self,
        session_id: &str,
        outcome: Option<cas_types::SessionOutcome>,
        friction_score: Option<f32>,
        delight_count: Option<u32>,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        // Single UPDATE using COALESCE to only set provided values,
        // replacing up to 3 separate UPDATEs.
        conn.execute(
            "UPDATE sessions SET
                outcome = COALESCE(?1, outcome),
                friction_score = COALESCE(?2, friction_score),
                delight_count = COALESCE(?3, delight_count)
             WHERE session_id = ?4",
            params![
                outcome.map(|o| o.to_string()),
                friction_score.map(|s| s.clamp(0.0, 1.0) as f64),
                delight_count.map(|c| c as i32),
                session_id,
            ],
        )?;

        Ok(())
    }

    // ========================================================================
    // Signals Aggregation Queries (Factory Signals)
    // ========================================================================

    /// Get friction events summary for a time period
    ///
    /// Returns: Vec<(friction_type, count, avg_severity)>
    pub fn friction_summary(&self, days: i64) -> Result<Vec<(String, i64, f64)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = Utc::now() - chrono::Duration::days(days);

        let mut stmt = conn.prepare_cached(
            "SELECT
                json_extract(metadata, '$.friction_type') as friction_type,
                COUNT(*) as count,
                AVG(CAST(json_extract(metadata, '$.severity') AS REAL)) as avg_severity
             FROM events
             WHERE event_type = 'friction_detected'
               AND created_at >= ?1
               AND json_extract(metadata, '$.friction_type') IS NOT NULL
             GROUP BY friction_type
             ORDER BY count DESC",
        )?;

        let results = stmt
            .query_map(params![cutoff.to_rfc3339()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Get sessions with high friction (friction count > threshold)
    ///
    /// Returns: Vec<(session_id, friction_count, avg_severity, outcome)>
    #[allow(clippy::type_complexity)]
    pub fn high_friction_sessions(
        &self,
        days: i64,
        threshold: usize,
        limit: usize,
    ) -> Result<Vec<(String, i64, f64, Option<String>)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = Utc::now() - chrono::Duration::days(days);

        let mut stmt = conn.prepare_cached(
            "SELECT
                e.session_id,
                COUNT(*) as friction_count,
                AVG(CAST(json_extract(e.metadata, '$.severity') AS REAL)) as avg_severity,
                s.outcome
             FROM events e
             LEFT JOIN sessions s ON e.session_id = s.session_id
             WHERE e.event_type = 'friction_detected'
               AND e.created_at >= ?1
               AND e.session_id IS NOT NULL
             GROUP BY e.session_id
             HAVING friction_count > ?2
             ORDER BY friction_count DESC
             LIMIT ?3",
        )?;

        let results = stmt
            .query_map(
                params![cutoff.to_rfc3339(), threshold as i64, limit as i64],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Get session outcome distribution for a time period
    ///
    /// Returns: Vec<(outcome, count, percentage)>
    pub fn outcome_summary(&self, days: i64) -> Result<Vec<(String, i64, f64)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = Utc::now() - chrono::Duration::days(days);

        // Single query using window function to compute percentage inline,
        // avoiding a separate COUNT(*) query that re-reads the same rows.
        let mut stmt = conn.prepare_cached(
            "SELECT
                outcome,
                COUNT(*) as count,
                COUNT(*) * 100.0 / SUM(COUNT(*)) OVER () as pct
             FROM sessions
             WHERE ended_at >= ?1
               AND outcome IS NOT NULL
             GROUP BY outcome
             ORDER BY count DESC",
        )?;

        let results = stmt
            .query_map(params![cutoff.to_rfc3339()], |row| {
                let outcome: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                let percentage: f64 = row.get(2)?;
                Ok((outcome, count, percentage))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Get correlation between friction and session outcomes
    ///
    /// Returns: Vec<(outcome, avg_friction_score, avg_friction_count, session_count)>
    pub fn outcome_correlation(&self, days: i64) -> Result<Vec<(String, f64, f64, i64)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = Utc::now() - chrono::Duration::days(days);

        let mut stmt = conn.prepare_cached(
            "SELECT
                s.outcome,
                AVG(COALESCE(s.friction_score, 0.0)) as avg_friction_score,
                AVG(COALESCE(friction_counts.count, 0)) as avg_friction_count,
                COUNT(DISTINCT s.session_id) as session_count
             FROM sessions s
             LEFT JOIN (
                SELECT session_id, COUNT(*) as count
                FROM events
                WHERE event_type = 'friction_detected'
                GROUP BY session_id
             ) friction_counts ON s.session_id = friction_counts.session_id
             WHERE s.ended_at >= ?1
               AND s.outcome IS NOT NULL
             GROUP BY s.outcome
             ORDER BY avg_friction_score DESC",
        )?;

        let results = stmt
            .query_map(params![cutoff.to_rfc3339()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Get detailed friction breakdown by type for a time period
    ///
    /// Returns: Vec<(friction_type, count, avg_severity, affected_sessions)>
    pub fn friction_by_type(&self, days: i64) -> Result<Vec<(String, i64, f64, i64)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = Utc::now() - chrono::Duration::days(days);

        let mut stmt = conn.prepare_cached(
            "SELECT
                json_extract(metadata, '$.friction_type') as friction_type,
                COUNT(*) as count,
                AVG(CAST(json_extract(metadata, '$.severity') AS REAL)) as avg_severity,
                COUNT(DISTINCT session_id) as affected_sessions
             FROM events
             WHERE event_type = 'friction_detected'
               AND created_at >= ?1
               AND json_extract(metadata, '$.friction_type') IS NOT NULL
             GROUP BY friction_type
             ORDER BY count DESC",
        )?;

        let results = stmt
            .query_map(params![cutoff.to_rfc3339()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }
}

pub struct SqliteRuleStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRuleStore {
    /// Open or create a SQLite rule store (uses same database as entry store)
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        Ok(Self { conn })
    }

    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    fn parse_scope(s: Option<String>) -> Scope {
        s.and_then(|t| t.parse().ok()).unwrap_or(Scope::Project)
    }

    fn parse_source_ids(s: Option<String>) -> Vec<String> {
        s.map(|ids| {
            if ids.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&ids)
                    .unwrap_or_else(|_| ids.split(',').map(|t| t.trim().to_string()).collect())
            }
        })
        .unwrap_or_default()
    }

    fn source_ids_to_string(ids: &[String]) -> String {
        if ids.is_empty() {
            String::new()
        } else {
            serde_json::to_string(ids).unwrap_or_default()
        }
    }

    fn parse_tags(s: Option<String>) -> Vec<String> {
        s.map(|tags| {
            if tags.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&tags)
                    .unwrap_or_else(|_| tags.split(',').map(|t| t.trim().to_string()).collect())
            }
        })
        .unwrap_or_default()
    }

    fn tags_to_string(tags: &[String]) -> String {
        if tags.is_empty() {
            String::new()
        } else {
            serde_json::to_string(tags).unwrap_or_default()
        }
    }

    /// List only proven rules (status = 'proven')
    pub fn list_proven(&self) -> Result<Vec<Rule>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, created, source_ids, helpful_count, harmful_count,
             tags, paths, content, status, last_accessed, review_after, hook_command,
             category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share
             FROM rules WHERE status = 'proven' ORDER BY priority ASC, created DESC",
        )?;

        let rules = stmt
            .query_map([], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    scope: Self::parse_scope(row.get(15)?),
                    created: Self::parse_datetime(&row.get::<_, String>(1)?)
                        .unwrap_or_else(Utc::now),
                    source_ids: Self::parse_source_ids(row.get(2)?),
                    helpful_count: row.get(3)?,
                    harmful_count: row.get(4)?,
                    tags: Self::parse_tags(row.get(5)?),
                    paths: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    content: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(RuleStatus::Draft),
                    last_accessed: row
                        .get::<_, Option<String>>(9)?
                        .and_then(|s| Self::parse_datetime(&s)),
                    review_after: row
                        .get::<_, Option<String>>(10)?
                        .and_then(|s| Self::parse_datetime(&s)),
                    hook_command: row.get(11)?,
                    category: row
                        .get::<_, Option<String>>(12)?
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    priority: row.get::<_, Option<u8>>(13)?.unwrap_or(2),
                    surface_count: row.get::<_, Option<i32>>(14)?.unwrap_or(0),
                    auto_approve_tools: row.get(16)?,
                    auto_approve_paths: row.get(17)?,
                    team_id: row.get(18)?,
                    share: row
                        .get::<_, Option<String>>(19)?
                        .as_deref()
                        .and_then(|s| s.parse().ok()),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rules)
    }

    /// List critical rules (priority = 0, proven or draft)
    pub fn list_critical(&self) -> Result<Vec<Rule>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, created, source_ids, helpful_count, harmful_count,
             tags, paths, content, status, last_accessed, review_after, hook_command,
             category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share
             FROM rules WHERE priority = 0 AND status IN ('proven', 'draft') ORDER BY created DESC",
        )?;

        let rules = stmt
            .query_map([], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    scope: Self::parse_scope(row.get(15)?),
                    created: Self::parse_datetime(&row.get::<_, String>(1)?)
                        .unwrap_or_else(Utc::now),
                    source_ids: Self::parse_source_ids(row.get(2)?),
                    helpful_count: row.get(3)?,
                    harmful_count: row.get(4)?,
                    tags: Self::parse_tags(row.get(5)?),
                    paths: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    content: row.get(7)?,
                    status: row
                        .get::<_, String>(8)?
                        .parse()
                        .unwrap_or(RuleStatus::Draft),
                    last_accessed: row
                        .get::<_, Option<String>>(9)?
                        .and_then(|s| Self::parse_datetime(&s)),
                    review_after: row
                        .get::<_, Option<String>>(10)?
                        .and_then(|s| Self::parse_datetime(&s)),
                    hook_command: row.get(11)?,
                    category: row
                        .get::<_, Option<String>>(12)?
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    priority: row.get::<_, Option<u8>>(13)?.unwrap_or(2),
                    surface_count: row.get::<_, Option<i32>>(14)?.unwrap_or(0),
                    auto_approve_tools: row.get(16)?,
                    auto_approve_paths: row.get(17)?,
                    team_id: row.get(18)?,
                    share: row
                        .get::<_, Option<String>>(19)?
                        .as_deref()
                        .and_then(|s| s.parse().ok()),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rules)
    }
}

mod rule_store_trait;
mod store_entry_crud;
mod store_entry_indexing;
mod store_entry_queries;
mod store_trait;
#[cfg(test)]
mod tests;
