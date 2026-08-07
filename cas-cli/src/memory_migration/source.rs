//! Read-only extraction of legacy `entries` rows (cas-f4c1 / EPIC cas-b129).
//!
//! Two hard requirements from the M2 spec live here:
//!
//! - **§11 assert 6** — extraction must NOT go through `Store::list()`, which
//!   caps at `LIMIT 10000` (`store_entry_crud.rs:267`). A migration that reads
//!   through it silently drops every row past the cap. This module uses keyset
//!   pagination over the primary key instead, which is also stable against
//!   concurrent inserts in a way `OFFSET` is not.
//! - **read-only by construction** — `SqliteStore::open` hands out a read-WRITE
//!   connection (`shared_db.rs:45`). The migration reads its sources through a
//!   dedicated `SQLITE_OPEN_READ_ONLY` handle (precedent:
//!   `cli/foreign_rows.rs:436`) so a bug cannot mutate the corpus it is
//!   measuring, and a missing file is an error rather than a freshly created
//!   empty database that would report "nothing to migrate".

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};

/// Columns the migration reads, in projection order.
///
/// `share` and `scope` are deliberately absent: both are `deliberately-leave`
/// (§5.1, §5.12) and `scope` is known-false on every GLOBAL row. Not selecting
/// them makes "M3 MUST NOT synthesize a share value" (Rule S1) structural
/// rather than aspirational — the value is never in memory to begin with.
const PROJECTION: &str = "id, type, tags, created, helpful_count, harmful_count, last_accessed, \
     title, content, archived, session_id, source_tool, observation_type, \
     stability, access_count, raw_content, compressed, memory_tier, importance, \
     valid_from, valid_until, review_after, last_reviewed, belief_type, \
     confidence, domain, branch, team_id, updated_at";

/// One legacy `entries` row, exactly as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyRow {
    pub id: String,
    pub entry_type: String,
    pub tags: Vec<String>,
    pub created: String,
    pub helpful_count: i64,
    pub harmful_count: i64,
    pub last_accessed: Option<String>,
    pub title: Option<String>,
    pub content: String,
    pub archived: i64,
    pub session_id: Option<String>,
    pub source_tool: Option<String>,
    pub observation_type: Option<String>,
    pub stability: f64,
    pub access_count: i64,
    pub raw_content: Option<String>,
    pub compressed: i64,
    pub memory_tier: String,
    pub importance: f64,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub review_after: Option<String>,
    pub last_reviewed: Option<String>,
    pub belief_type: String,
    pub confidence: f64,
    pub domain: Option<String>,
    pub branch: Option<String>,
    pub team_id: Option<String>,
    pub updated_at: Option<String>,
}

/// Mirror of `SqliteStore::parse_tags` (`sqlite/mod.rs:197`): JSON array when
/// the column holds one, comma-separated fallback otherwise.
fn parse_tags(raw: Option<String>) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    if raw.is_empty() {
        return Vec::new();
    }
    serde_json::from_str(&raw)
        .unwrap_or_else(|_| raw.split(',').map(|t| t.trim().to_string()).collect())
}

impl LegacyRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            entry_type: row
                .get::<_, Option<String>>(1)?
                .unwrap_or_else(|| "learning".into()),
            tags: parse_tags(row.get(2)?),
            created: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            helpful_count: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            harmful_count: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
            last_accessed: row.get(6)?,
            title: row.get(7)?,
            content: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
            archived: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
            session_id: row.get(10)?,
            source_tool: row.get(11)?,
            observation_type: row.get(12)?,
            stability: row.get::<_, Option<f64>>(13)?.unwrap_or(0.5),
            access_count: row.get::<_, Option<i64>>(14)?.unwrap_or(0),
            raw_content: row.get(15)?,
            compressed: row.get::<_, Option<i64>>(16)?.unwrap_or(0),
            memory_tier: row
                .get::<_, Option<String>>(17)?
                .unwrap_or_else(|| "working".into()),
            importance: row.get::<_, Option<f64>>(18)?.unwrap_or(0.5),
            valid_from: row.get(19)?,
            valid_until: row.get(20)?,
            review_after: row.get(21)?,
            last_reviewed: row.get(22)?,
            belief_type: row
                .get::<_, Option<String>>(23)?
                .unwrap_or_else(|| "fact".into()),
            confidence: row.get::<_, Option<f64>>(24)?.unwrap_or(1.0),
            domain: row.get(25)?,
            branch: row.get(26)?,
            team_id: row.get(27)?,
            updated_at: row.get(28)?,
        })
    }

    /// Effective page title: `entries.title` when it has one, else the
    /// `preview(60)` fallback SessionStart already displays
    /// (`behavior.rs:309`, `build_start.rs:293`). §4.2.
    pub fn display_title(&self) -> String {
        match self.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => title.to_string(),
            _ => preview(&self.content, 60),
        }
    }

    // ── test constructors ───────────────────────────────────────────────
    // Public so the acceptance tests can exercise routing without standing up
    // a database for every single rule.

    pub fn for_test(id: &str, entry_type: &str, title: Option<&str>, content: &str) -> Self {
        Self {
            id: id.to_string(),
            entry_type: entry_type.to_string(),
            tags: Vec::new(),
            created: "2026-01-01T00:00:00Z".to_string(),
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            title: title.map(ToString::to_string),
            content: content.to_string(),
            archived: 0,
            session_id: None,
            source_tool: None,
            observation_type: None,
            stability: 0.5,
            access_count: 0,
            raw_content: None,
            compressed: 0,
            memory_tier: "working".to_string(),
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            belief_type: "fact".to_string(),
            confidence: 1.0,
            domain: None,
            branch: None,
            team_id: None,
            updated_at: None,
        }
    }

    pub fn with_memory_tier(mut self, tier: &str) -> Self {
        self.memory_tier = tier.to_string();
        self
    }

    pub fn with_belief(mut self, belief_type: &str, confidence: f64) -> Self {
        self.belief_type = belief_type.to_string();
        self.confidence = confidence;
        self
    }

    pub fn with_feedback(mut self, helpful: i64, harmful: i64) -> Self {
        self.helpful_count = helpful;
        self.harmful_count = harmful;
        self
    }

    /// Every carriable column set to a non-default value — the fixture the
    /// frontmatter tests use to prove Rule C2's key set is complete.
    pub fn fully_populated_for_test() -> Self {
        Self {
            id: "p-full".to_string(),
            entry_type: "context".to_string(),
            tags: vec!["alpha".to_string(), "beta: gamma".to_string()],
            created: "2024-03-04T05:06:07Z".to_string(),
            helpful_count: 7,
            harmful_count: 2,
            last_accessed: Some("2025-10-01T00:00:00Z".to_string()),
            title: Some("Full Row".to_string()),
            content: "a durable project fact".to_string(),
            archived: 1,
            session_id: Some("sess-1".to_string()),
            source_tool: Some("mcp".to_string()),
            observation_type: Some("general".to_string()),
            stability: 0.75,
            access_count: 42,
            raw_content: None,
            compressed: 0,
            memory_tier: "archive".to_string(),
            importance: 0.9,
            valid_from: Some("2024-01-01T00:00:00Z".to_string()),
            valid_until: Some("2027-01-01T00:00:00Z".to_string()),
            review_after: Some("2026-06-01T00:00:00Z".to_string()),
            last_reviewed: Some("2026-05-01T00:00:00Z".to_string()),
            belief_type: "hypothesis".to_string(),
            confidence: 0.4,
            domain: Some("build".to_string()),
            branch: Some("main".to_string()),
            team_id: Some("team-42".to_string()),
            updated_at: Some("2025-09-08T09:10:11Z".to_string()),
        }
    }
}

/// First `limit` characters of the content, on a character boundary.
pub fn preview(content: &str, limit: usize) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    trimmed
        .chars()
        .take(limit)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Open a legacy database read-only.
///
/// `SQLITE_OPEN_READ_ONLY` without `CREATE`, so a missing file errors instead of
/// materializing an empty database that would audit as "nothing to migrate".
pub fn open_read_only(db_path: &Path) -> Result<Connection> {
    if !db_path.exists() {
        anyhow::bail!("legacy database not found: {}", db_path.display());
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", db_path.display()))?;
    conn.busy_timeout(Duration::from_millis(500))?;
    Ok(conn)
}

/// Row count of `entries`. Used for the Rule D0 stability assert.
pub fn count_entries(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?)
}

/// Extract every `entries` row via keyset pagination on the primary key.
///
/// Never `Store::list()` (§11 assert 6) and never `OFFSET`: keyset paging is
/// the only form that cannot skip or duplicate a row when the source database
/// is written to mid-scan, which it demonstrably is (Rule D0).
pub fn extract_all(conn: &Connection, page_size: usize) -> Result<Vec<LegacyRow>> {
    let page_size = page_size.max(1);
    let sql = format!("SELECT {PROJECTION} FROM entries WHERE id > ?1 ORDER BY id ASC LIMIT ?2");
    let mut stmt = conn.prepare(&sql)?;
    let mut out: Vec<LegacyRow> = Vec::new();
    let mut cursor = String::new();
    loop {
        let batch = stmt
            .query_map(params![cursor, page_size as i64], LegacyRow::from_row)?
            // No `.flatten()`: a row that fails to decode must abort the
            // migration, never silently shrink the corpus.
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(last) = batch.last() else { break };
        cursor = last.id.clone();
        let full = batch.len() == page_size;
        out.extend(batch);
        if !full {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_falls_back_on_the_untitled_two_thirds() {
        assert_eq!(preview("  short  ", 60), "short");
        let long = "x".repeat(100);
        assert_eq!(preview(&long, 60).len(), 60);
    }

    #[test]
    fn preview_never_splits_a_multibyte_character() {
        let content = "é".repeat(100);
        let cut = preview(&content, 60);
        assert_eq!(cut.chars().count(), 60);
    }

    #[test]
    fn tags_parse_from_json_and_from_the_comma_fallback() {
        assert_eq!(parse_tags(Some(r#"["a","b"]"#.into())), vec!["a", "b"]);
        assert_eq!(parse_tags(Some("a, b".into())), vec!["a", "b"]);
        assert!(parse_tags(Some(String::new())).is_empty());
        assert!(parse_tags(None).is_empty());
    }

    #[test]
    fn share_and_scope_are_not_in_the_projection() {
        // Rule S1 / §5.12 enforced structurally: the values never enter memory.
        assert!(!PROJECTION.contains("share"));
        assert!(!PROJECTION.split(", ").any(|c| c.trim() == "scope"));
    }
}
