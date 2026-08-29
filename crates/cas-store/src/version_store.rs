//! Shared schema and records for rule/skill lifecycle history.

use chrono::{DateTime, Utc};
use cas_types::{RuleStatus, SkillStatus};

/// Statements used by the numbered migration and lazy SQLite store bootstrap.
pub const RULE_VERSIONS_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS rule_versions (id INTEGER PRIMARY KEY AUTOINCREMENT, rule_id TEXT NOT NULL, version INTEGER NOT NULL, snapshot_json TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL, changed_by TEXT, changed_at TEXT NOT NULL, change_note TEXT NOT NULL, UNIQUE(rule_id, version))",
    "CREATE INDEX IF NOT EXISTS idx_rule_versions_rule ON rule_versions(rule_id, version DESC)",
];

/// Statements used by the numbered migration and lazy SQLite store bootstrap.
pub const SKILL_VERSIONS_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS skill_versions (id INTEGER PRIMARY KEY AUTOINCREMENT, skill_id TEXT NOT NULL, version INTEGER NOT NULL, snapshot_json TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL, status TEXT NOT NULL, changed_by TEXT, changed_at TEXT NOT NULL, change_note TEXT NOT NULL, UNIQUE(skill_id, version))",
    "CREATE INDEX IF NOT EXISTS idx_skill_versions_skill ON skill_versions(skill_id, version DESC)",
];

/// Combined statements for migration m237.
pub const RULE_AND_SKILL_VERSIONS_SCHEMA_STATEMENTS: &[&str] = &[
    RULE_VERSIONS_SCHEMA_STATEMENTS[0],
    RULE_VERSIONS_SCHEMA_STATEMENTS[1],
    SKILL_VERSIONS_SCHEMA_STATEMENTS[0],
    SKILL_VERSIONS_SCHEMA_STATEMENTS[1],
];

/// A prior rule state captured before a mutation.
#[derive(Debug, Clone)]
pub struct RuleVersion {
    pub id: i64,
    pub rule_id: String,
    pub version: i64,
    pub content: String,
    pub status: RuleStatus,
    pub changed_by: Option<String>,
    pub changed_at: DateTime<Utc>,
    pub change_note: String,
    pub snapshot_json: String,
}

/// A prior skill state captured before a mutation.
#[derive(Debug, Clone)]
pub struct SkillVersion {
    pub id: i64,
    pub skill_id: String,
    pub version: i64,
    pub name: String,
    pub description: String,
    pub status: SkillStatus,
    pub changed_by: Option<String>,
    pub changed_at: DateTime<Utc>,
    pub change_note: String,
    pub snapshot_json: String,
}

/// Resolve a useful actor identity when callers do not supply one explicitly.
pub(crate) fn default_changed_by() -> Option<String> {
    ["CAS_AGENT_NAME", "CAS_AGENT_ID", "USER"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.trim().is_empty()))
}

pub(crate) fn parse_datetime(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
