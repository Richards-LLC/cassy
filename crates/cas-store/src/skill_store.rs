//! SQLite-based skill storage

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Per-process counter mixed into `generate_hash_id` so that two back-to-back
/// calls within the same nanosecond (same `timestamp_nanos + pid`) always
/// produce different hashes and therefore different candidate IDs. Fixes the
/// UNIQUE-constraint flake in `test_skill_search` (cas-6c0a).
static SKILL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

use crate::error::StoreError;
use crate::version_store::{
    SKILL_VERSIONS_SCHEMA_STATEMENTS, SkillVersion, default_changed_by, parse_datetime,
};
use crate::{Result, SkillStore};
use cas_types::{Scope, Skill, SkillHooks, SkillStatus, SkillType};

/// SQLite DDL for the `skills` table and its indexes.
///
/// Re-exported via `cas_store::SKILL_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base table before applying ALTER migrations
/// against subsystems whose tables were historically created lazily by
/// `SqliteSkillStore::init`. See cas-bdb9 / EPIC cas-9fdb.
pub const SKILL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    skill_type TEXT NOT NULL DEFAULT 'command',
    invocation TEXT NOT NULL DEFAULT '',
    parameters_schema TEXT NOT NULL DEFAULT '',
    example TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'enabled',
    tags TEXT NOT NULL DEFAULT '[]',
    summary TEXT NOT NULL DEFAULT '',
    -- Validation columns
    preconditions TEXT NOT NULL DEFAULT '[]',
    postconditions TEXT NOT NULL DEFAULT '[]',
    validation_script TEXT NOT NULL DEFAULT '',
    -- Invokable skill support
    invokable INTEGER NOT NULL DEFAULT 0,
    argument_hint TEXT NOT NULL DEFAULT '',
    -- Claude Code frontmatter fields (added for Claude Code compatibility)
    context_mode TEXT,
    agent_type TEXT,
    allowed_tools TEXT NOT NULL DEFAULT '[]',
    -- Skill-scoped hooks (Claude Code 2.1.0+)
    hooks TEXT,
    -- Disable model invocation (Claude Code 2.1.3+)
    disable_model_invocation INTEGER NOT NULL DEFAULT 0,
    -- Disallowed tools enforcement (Claude Code 2.1.152+, cas-5be8)
    disallowed_tools TEXT NOT NULL DEFAULT '[]',
    -- Usage tracking
    usage_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_used TEXT,
    -- Team collaboration
    team_id TEXT,
    -- Team-promotion share override (private | team)
    share TEXT,
    -- Skill provenance (JSON array of source entry IDs)
    source_ids TEXT
);

CREATE INDEX IF NOT EXISTS idx_skills_status ON skills(status);
CREATE INDEX IF NOT EXISTS idx_skills_name ON skills(name);
"#;

// NOTE: Column migrations are now handled by `cas update --schema-only`
// See cas-cli/src/migration/migrations.rs for migration definitions (IDs 71-76)

/// SQLite-based skill store
pub struct SqliteSkillStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSkillStore {
    fn insert_version(
        tx: &Transaction<'_>,
        skill_id: &str,
        snapshot_json: &str,
        name: &str,
        description: &str,
        status: SkillStatus,
        changed_by: &Option<String>,
        changed_at: &str,
        change_note: &str,
        operation: &str,
    ) -> Result<()> {
        let next_version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM skill_versions WHERE skill_id = ?1",
            params![skill_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO skill_versions
             (skill_id, version, snapshot_json, name, description, status, changed_by, changed_at, change_note, operation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                skill_id,
                next_version,
                snapshot_json,
                name,
                description,
                status.to_string(),
                changed_by,
                changed_at,
                change_note,
                operation,
            ],
        )?;
        Ok(())
    }

    /// Open or create a SQLite skill store
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

    fn parse_tags(s: &str) -> Vec<String> {
        if s.is_empty() || s == "[]" {
            return Vec::new();
        }
        serde_json::from_str(s).unwrap_or_default()
    }

    fn tags_to_string(tags: &[String]) -> String {
        if tags.is_empty() {
            "[]".to_string()
        } else {
            serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string())
        }
    }

    /// Generate a hash-based ID like cas-sk01
    fn generate_hash_id(&self) -> Result<String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Mix a monotonically-increasing counter so that two back-to-back calls
        // within the same nanosecond (same timestamp + same pid) produce a
        // different hash and therefore a different candidate ID (cas-6c0a).
        let seq = SKILL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

        let mut hasher = DefaultHasher::new();
        Utc::now().timestamp_nanos_opt().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        seq.hash(&mut hasher);

        let hash = hasher.finish();
        let chars: Vec<char> = format!("{hash:016x}").chars().collect();

        // Try sk + 2-char, then sk + 3-char, then sk + 4-char IDs
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        for len in 2..=4 {
            let id = format!("cas-sk{}", chars[..len].iter().collect::<String>());
            let exists: bool = conn
                .query_row("SELECT 1 FROM skills WHERE id = ?", params![&id], |_| {
                    Ok(true)
                })
                .optional()?
                .unwrap_or(false);

            if !exists {
                return Ok(id);
            }
        }

        // Fallback to longer hash
        Ok(format!("cas-sk{}", &chars[..6].iter().collect::<String>()))
    }

    fn parse_hooks(s: &str) -> Option<SkillHooks> {
        if s.is_empty() {
            return None;
        }
        serde_json::from_str(s).ok()
    }

    fn hooks_to_string(hooks: &Option<SkillHooks>) -> Option<String> {
        hooks.as_ref().and_then(|h| {
            if h.is_empty() {
                None
            } else {
                serde_json::to_string(h).ok()
            }
        })
    }

    fn skill_from_row(row: &rusqlite::Row) -> rusqlite::Result<Skill> {
        Ok(Skill {
            scope: Scope::default(),
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get::<_, String>(2)?,
            skill_type: row
                .get::<_, String>(3)?
                .parse()
                .unwrap_or(SkillType::Command),
            invocation: row.get::<_, String>(4)?,
            parameters_schema: row.get::<_, String>(5)?,
            example: row.get::<_, String>(6)?,
            preconditions: Self::parse_tags(&row.get::<_, String>(7).unwrap_or_default()),
            postconditions: Self::parse_tags(&row.get::<_, String>(8).unwrap_or_default()),
            validation_script: row.get::<_, String>(9).unwrap_or_default(),
            status: row
                .get::<_, String>(10)?
                .parse()
                .unwrap_or(SkillStatus::Enabled),
            tags: Self::parse_tags(&row.get::<_, String>(11)?),
            summary: row.get::<_, String>(12).unwrap_or_default(),
            invokable: row.get::<_, i32>(17).unwrap_or(0) != 0,
            argument_hint: row.get::<_, String>(18).unwrap_or_default(),
            // Claude Code frontmatter fields (columns 19-22)
            context_mode: row.get::<_, Option<String>>(19).unwrap_or(None),
            agent_type: row.get::<_, Option<String>>(20).unwrap_or(None),
            allowed_tools: Self::parse_tags(&row.get::<_, String>(21).unwrap_or_default()),
            // Hooks column (22) - Claude Code 2.1.0+
            hooks: row
                .get::<_, Option<String>>(22)
                .unwrap_or(None)
                .and_then(|s| Self::parse_hooks(&s)),
            // Disable model invocation (23) - Claude Code 2.1.3+
            disable_model_invocation: row.get::<_, i32>(23).unwrap_or(0) != 0,
            usage_count: row.get::<_, i32>(13)?,
            created_at: Self::parse_datetime(&row.get::<_, String>(14)?).unwrap_or_else(Utc::now),
            updated_at: Self::parse_datetime(&row.get::<_, String>(15)?).unwrap_or_else(Utc::now),
            last_used: row
                .get::<_, Option<String>>(16)?
                .and_then(|s| Self::parse_datetime(&s)),
            team_id: row.get(24)?,
            share: row
                .get::<_, Option<String>>(25)?
                .as_deref()
                .and_then(|s| s.parse().ok()),
            // Disallowed tools (26) - Claude Code 2.1.152+, cas-5be8
            disallowed_tools: Self::parse_tags(&row.get::<_, String>(26).unwrap_or_default()),
            // Source entry IDs (27)
            source_ids: Self::parse_tags(&row.get::<_, String>(27).unwrap_or_default()),
        })
    }

    fn update_recorded(
        &self,
        skill: &Skill,
        changed_by: Option<&str>,
        change_note: Option<&str>,
        operation: &str,
    ) -> Result<()> {
        let previous = self.get(&skill.id)?;
        let snapshot_json = serde_json::to_string(&previous).map_err(|error| {
            StoreError::Parse(format!("failed to serialize skill history: {error}"))
        })?;
        let changed_by = changed_by
            .map(ToOwned::to_owned)
            .or_else(default_changed_by);
        let change_note = change_note.unwrap_or("update");
        let changed_at = Utc::now().to_rfc3339();

        let mut conn = crate::shared_db::lock_connection(&self.conn)?;
        let tx = conn.transaction()?;
        Self::insert_version(
            &tx,
            &skill.id,
            &snapshot_json,
            &previous.name,
            &previous.description,
            previous.status,
            &changed_by,
            &changed_at,
            change_note,
            operation,
        )?;
        let rows = tx.execute(
            "UPDATE skills SET name = ?1, description = ?2, skill_type = ?3,
             invocation = ?4, parameters_schema = ?5, example = ?6,
             preconditions = ?7, postconditions = ?8, validation_script = ?9,
             status = ?10, tags = ?11, summary = ?12, usage_count = ?13,
             updated_at = ?14, last_used = ?15, invokable = ?16, argument_hint = ?17,
             context_mode = ?18, agent_type = ?19, allowed_tools = ?20, hooks = ?21,
             disable_model_invocation = ?22, team_id = ?23, share = ?24,
             disallowed_tools = ?25, source_ids = ?26
             WHERE id = ?27",
            params![
                skill.name,
                skill.description,
                skill.skill_type.to_string(),
                skill.invocation,
                skill.parameters_schema,
                skill.example,
                Self::tags_to_string(&skill.preconditions),
                Self::tags_to_string(&skill.postconditions),
                skill.validation_script,
                skill.status.to_string(),
                Self::tags_to_string(&skill.tags),
                skill.summary,
                skill.usage_count,
                Utc::now().to_rfc3339(),
                skill.last_used.map(|t| t.to_rfc3339()),
                skill.invokable as i32,
                skill.argument_hint,
                skill.context_mode,
                skill.agent_type,
                Self::tags_to_string(&skill.allowed_tools),
                Self::hooks_to_string(&skill.hooks),
                skill.disable_model_invocation as i32,
                skill.team_id,
                skill.share.as_ref().map(|s| s.to_string()),
                Self::tags_to_string(&skill.disallowed_tools),
                Self::tags_to_string(&skill.source_ids),
                skill.id,
            ],
        )?;
        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "skill not found: {}",
                skill.id
            )));
        }
        tx.commit()?;
        Ok(())
    }

    fn delete_recorded(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let previous = self.get(id)?;
        if previous.status == SkillStatus::Retired {
            return Ok(());
        }
        let mut retired = previous;
        retired.status = SkillStatus::Retired;
        self.update_recorded(
            &retired,
            changed_by,
            Some(change_note.unwrap_or("tombstone delete")),
            "delete",
        )
    }

    pub fn list_versions(&self, id: &str) -> Result<Vec<SkillVersion>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, skill_id, version, snapshot_json, name, description, status,
                    changed_by, changed_at, change_note, operation
             FROM skill_versions WHERE skill_id = ?1 ORDER BY version DESC",
        )?;
        let versions = stmt
            .query_map(params![id], |row| {
                Ok(SkillVersion {
                    id: row.get(0)?,
                    skill_id: row.get(1)?,
                    version: row.get(2)?,
                    snapshot_json: row.get(3)?,
                    name: row.get(4)?,
                    description: row.get(5)?,
                    status: row
                        .get::<_, String>(6)?
                        .parse()
                        .unwrap_or(SkillStatus::Enabled),
                    changed_by: row.get(7)?,
                    changed_at: parse_datetime(&row.get::<_, String>(8)?),
                    change_note: row.get(9)?,
                    operation: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(versions)
    }

    pub fn restore_version(
        &self,
        id: &str,
        version: Option<i64>,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let snapshot_json: String = match version {
            Some(version) => conn
                .query_row(
                    "SELECT snapshot_json FROM skill_versions WHERE skill_id = ?1 AND version = ?2",
                    params![id, version],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::Parse(format!("skill version not found: {id} v{version}")))?,
            None => conn
                .query_row(
                    "SELECT snapshot_json FROM skill_versions WHERE skill_id = ?1 ORDER BY version DESC LIMIT 1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::Parse(format!("no skill history for: {id}")))?,
        };
        drop(conn);

        let restored: Skill = serde_json::from_str(&snapshot_json).map_err(|error| {
            StoreError::Parse(format!("invalid skill history for {id}: {error}"))
        })?;
        if restored.id != id {
            return Err(StoreError::Parse(format!(
                "skill history ID mismatch for: {id}"
            )));
        }
        let default_note;
        let note = match (change_note, version) {
            (Some(note), _) => Some(note),
            (None, Some(version)) => {
                default_note = format!("restore version {version}");
                Some(default_note.as_str())
            }
            (None, None) => Some("restore latest version"),
        };
        self.update_recorded(&restored, changed_by, note, "restore")
    }
}

impl SkillStore for SqliteSkillStore {
    fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(SKILL_SCHEMA)?;
        for statement in SKILL_VERSIONS_SCHEMA_STATEMENTS {
            conn.execute(statement, [])?;
        }
        // NOTE: Column migrations are handled by `cas update --schema-only`
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        self.generate_hash_id()
    }

    fn add(&self, skill: &Skill) -> Result<()> {
        let snapshot_json = serde_json::to_string(skill).map_err(|error| {
            StoreError::Parse(format!("failed to serialize skill history: {error}"))
        })?;
        let changed_by = default_changed_by();
        let changed_at = Utc::now().to_rfc3339();
        let mut conn = crate::shared_db::lock_connection(&self.conn)?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO skills (id, name, description, skill_type, invocation, parameters_schema,
             example, preconditions, postconditions, validation_script, status, tags, summary,
             usage_count, created_at, updated_at, last_used, invokable, argument_hint,
             context_mode, agent_type, allowed_tools, hooks, disable_model_invocation, team_id, share,
             disallowed_tools, source_ids)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
            params![
                skill.id,
                skill.name,
                skill.description,
                skill.skill_type.to_string(),
                skill.invocation,
                skill.parameters_schema,
                skill.example,
                Self::tags_to_string(&skill.preconditions),
                Self::tags_to_string(&skill.postconditions),
                skill.validation_script,
                skill.status.to_string(),
                Self::tags_to_string(&skill.tags),
                skill.summary,
                skill.usage_count,
                skill.created_at.to_rfc3339(),
                skill.updated_at.to_rfc3339(),
                skill.last_used.map(|t| t.to_rfc3339()),
                skill.invokable as i32,
                skill.argument_hint,
                skill.context_mode,
                skill.agent_type,
                Self::tags_to_string(&skill.allowed_tools),
                Self::hooks_to_string(&skill.hooks),
                skill.disable_model_invocation as i32,
                skill.team_id,
                skill.share.as_ref().map(|s| s.to_string()),
                Self::tags_to_string(&skill.disallowed_tools),
                Self::tags_to_string(&skill.source_ids),
            ],
        )?;
        Self::insert_version(
            &tx,
            &skill.id,
            &snapshot_json,
            &skill.name,
            &skill.description,
            skill.status,
            &changed_by,
            &changed_at,
            "create",
            "create",
        )?;
        tx.commit()?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Skill> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT id, name, description, skill_type, invocation, parameters_schema,
             example, preconditions, postconditions, validation_script, status, tags, summary,
             usage_count, created_at, updated_at, last_used, invokable, argument_hint,
             context_mode, agent_type, allowed_tools, hooks, disable_model_invocation, team_id, share,
             disallowed_tools, source_ids
             FROM skills WHERE id = ?",
            params![id],
            Self::skill_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("skill not found: {id}")))
    }

    fn update(&self, skill: &Skill) -> Result<()> {
        self.update_recorded(skill, None, None, "update")
    }

    fn update_with_metadata(
        &self,
        skill: &Skill,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.update_recorded(skill, changed_by, change_note, "update")
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.delete_recorded(id, None, None)
    }

    fn delete_with_metadata(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.delete_recorded(id, changed_by, change_note)
    }

    fn list_versions(&self, id: &str) -> Result<Vec<SkillVersion>> {
        SqliteSkillStore::list_versions(self, id)
    }

    fn restore_version(
        &self,
        id: &str,
        version: Option<i64>,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        SqliteSkillStore::restore_version(self, id, version, changed_by, change_note)
    }

    fn list(&self, status: Option<SkillStatus>) -> Result<Vec<Skill>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let (sql, params): (&str, Vec<String>) = match status {
            Some(s) => (
                "SELECT id, name, description, skill_type, invocation, parameters_schema,
                 example, preconditions, postconditions, validation_script, status, tags, summary,
                 usage_count, created_at, updated_at, last_used, invokable, argument_hint,
                 context_mode, agent_type, allowed_tools, hooks, disable_model_invocation, team_id, share,
                 disallowed_tools, source_ids
                 FROM skills WHERE status = ? ORDER BY name",
                vec![s.to_string()],
            ),
            None => (
                "SELECT id, name, description, skill_type, invocation, parameters_schema,
                 example, preconditions, postconditions, validation_script, status, tags, summary,
                 usage_count, created_at, updated_at, last_used, invokable, argument_hint,
                 context_mode, agent_type, allowed_tools, hooks, disable_model_invocation, team_id, share,
                 disallowed_tools, source_ids
                 FROM skills ORDER BY name",
                vec![],
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let skills = if params.is_empty() {
            stmt.query_map([], Self::skill_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![params[0]], Self::skill_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(skills)
    }

    fn list_enabled(&self) -> Result<Vec<Skill>> {
        self.list(Some(SkillStatus::Enabled))
    }

    fn search(&self, query: &str) -> Result<Vec<Skill>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let pattern = format!("%{query}%");

        let mut stmt = conn.prepare_cached(
            "SELECT id, name, description, skill_type, invocation, parameters_schema,
             example, preconditions, postconditions, validation_script, status, tags, summary,
             usage_count, created_at, updated_at, last_used, invokable, argument_hint,
             context_mode, agent_type, allowed_tools, hooks, disable_model_invocation, team_id, share,
             disallowed_tools, source_ids
             FROM skills
             WHERE name LIKE ?1 OR description LIKE ?1 OR tags LIKE ?1 OR summary LIKE ?1
             ORDER BY usage_count DESC, name",
        )?;

        let skills = stmt
            .query_map(params![&pattern], Self::skill_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(skills)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::skill_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqliteSkillStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteSkillStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    #[test]
    fn test_skill_crud() {
        let (_temp, store) = create_test_store();

        // Create skill
        let id = store.generate_id().unwrap();
        let mut skill = Skill::new(id.clone(), "Test Skill".to_string());
        skill.description = "A test skill".to_string();
        skill.skill_type = SkillType::Command;
        skill.invocation = "echo hello".to_string();
        skill.tags = vec!["test".to_string()];
        store.add(&skill).unwrap();

        // Get skill
        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.name, "Test Skill");
        assert_eq!(retrieved.description, "A test skill");
        assert_eq!(retrieved.tags, vec!["test"]);

        // Update skill
        skill.description = "Updated description".to_string();
        skill.usage_count = 5;
        store.update(&skill).unwrap();

        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.description, "Updated description");
        assert_eq!(retrieved.usage_count, 5);

        // List skills
        let all_skills = store.list(None).unwrap();
        assert_eq!(all_skills.len(), 1);

        let enabled = store.list_enabled().unwrap();
        assert_eq!(enabled.len(), 1);

        // Delete skill
        store.delete(&id).unwrap();
        assert_eq!(store.get(&id).unwrap().status, SkillStatus::Retired);
        assert!(store.list_enabled().unwrap().is_empty());
    }

    #[test]
    fn test_skill_search() {
        let (_temp, store) = create_test_store();

        // Create skills
        let skill1 = Skill {
            id: store.generate_id().unwrap(),
            name: "File Search".to_string(),
            description: "Search for files by pattern".to_string(),
            tags: vec!["files".to_string(), "search".to_string()],
            ..Default::default()
        };
        let skill2 = Skill {
            id: store.generate_id().unwrap(),
            name: "Git Status".to_string(),
            description: "Check git repository status".to_string(),
            tags: vec!["git".to_string()],
            ..Default::default()
        };
        store.add(&skill1).unwrap();
        store.add(&skill2).unwrap();

        // Search by name
        let results = store.search("File").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "File Search");

        // Search by description
        let results = store.search("repository").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Git Status");

        // Search by tag
        let results = store.search("search").unwrap();
        assert_eq!(results.len(), 1);
    }

    /// cas-30af: skill updates retain prior snapshots and deletes are
    /// tombstones so a retired skill remains restorable in the database.
    #[test]
    fn test_skill_history_and_tombstone_delete() {
        let temp = TempDir::new().unwrap();
        let store = SqliteSkillStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let skill = Skill::new("skill-history-01".to_string(), "History Skill".to_string());
        store.add(&skill).unwrap();
        let mut updated = skill.clone();
        updated.description = "updated description".to_string();
        store
            .update_with_metadata(&updated, Some("test-actor"), Some("revise instructions"))
            .unwrap();

        let versions = store.list_versions("skill-history-01").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].description, "");
        assert_eq!(versions[0].operation, "update");
        assert_eq!(versions[0].changed_by.as_deref(), Some("test-actor"));
        assert_eq!(versions[0].change_note, "revise instructions");

        let conn = Connection::open(temp.path().join("cas.db")).unwrap();
        let prior_description: String = conn
            .query_row(
                "SELECT description FROM skill_versions WHERE skill_id = 'skill-history-01' ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(prior_description, "");

        store.delete("skill-history-01").unwrap();
        let retired = store.get("skill-history-01").unwrap();
        assert_eq!(retired.status.to_string(), "retired");
        let version_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM skill_versions WHERE skill_id = 'skill-history-01'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version_count, 3);
        let operation: String = conn
            .query_row(
                "SELECT operation FROM skill_versions WHERE skill_id = 'skill-history-01' AND version = 3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(operation, "delete");

        store
            .restore_version("skill-history-01", Some(1), Some("restorer"), None)
            .unwrap();
        let restored = store.get("skill-history-01").unwrap();
        assert_eq!(restored.description, "");
        assert_eq!(restored.status, SkillStatus::Enabled);
    }

    /// cas-ef20: creating a skill is an auditable lifecycle mutation too. The
    /// initial snapshot must be restorable and identify the create operation.
    #[test]
    fn test_skill_create_is_versioned() {
        let temp = TempDir::new().unwrap();
        let store = SqliteSkillStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let skill = Skill::new(
            "skill-create-history-01".to_string(),
            "Created Skill".to_string(),
        );
        store.add(&skill).unwrap();

        let versions = store.list_versions(&skill.id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[0].name, skill.name);
        assert_eq!(versions[0].status, skill.status);

        let conn = Connection::open(temp.path().join("cas.db")).unwrap();
        let operation: String = conn
            .query_row(
                "SELECT operation FROM skill_versions WHERE skill_id = ?1",
                [&skill.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(operation, "create");
    }

    #[test]
    fn test_skill_invokable() {
        let (_temp, store) = create_test_store();

        // Create invokable skill
        let id = store.generate_id().unwrap();
        let mut skill = Skill::new(id.clone(), "Task Creator".to_string());
        skill.description = "Create a task".to_string();
        skill.invokable = true;
        skill.argument_hint = "[title]".to_string();
        store.add(&skill).unwrap();

        // Retrieve and verify
        let retrieved = store.get(&id).unwrap();
        assert!(retrieved.invokable);
        assert_eq!(retrieved.argument_hint, "[title]");

        // Update invokable fields
        skill.argument_hint = "[title] [priority?]".to_string();
        store.update(&skill).unwrap();

        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.argument_hint, "[title] [priority?]");
    }

    // cas-5be8: disallowed_tools round-trip
    #[test]
    fn test_skill_disallowed_tools_roundtrip() {
        let (_temp, store) = create_test_store();

        let id = store.generate_id().unwrap();
        let mut skill = Skill::new(id.clone(), "Restricted Skill".to_string());
        skill.disallowed_tools = vec![
            "Write".to_string(),
            "Edit".to_string(),
            "NotebookEdit".to_string(),
        ];
        store.add(&skill).unwrap();

        let retrieved = store.get(&id).unwrap();
        assert_eq!(
            retrieved.disallowed_tools,
            vec!["Write", "Edit", "NotebookEdit"],
            "disallowed_tools must survive a DB round-trip"
        );
    }

    #[test]
    fn test_skill_disallowed_tools_empty_roundtrip() {
        let (_temp, store) = create_test_store();

        let id = store.generate_id().unwrap();
        let skill = Skill::new(id.clone(), "Open Skill".to_string());
        // disallowed_tools defaults to empty Vec
        store.add(&skill).unwrap();

        let retrieved = store.get(&id).unwrap();
        assert!(
            retrieved.disallowed_tools.is_empty(),
            "empty disallowed_tools must round-trip as empty"
        );
    }
}
