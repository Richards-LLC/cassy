//! SQLite-based spec storage

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{Scope, Spec, SpecStatus, SpecType};

const SPEC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS specs (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL DEFAULT 'project',
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    goals TEXT NOT NULL DEFAULT '[]',
    in_scope TEXT NOT NULL DEFAULT '[]',
    out_of_scope TEXT NOT NULL DEFAULT '[]',
    users TEXT NOT NULL DEFAULT '[]',
    technical_requirements TEXT NOT NULL DEFAULT '[]',
    acceptance_criteria TEXT NOT NULL DEFAULT '[]',
    design_notes TEXT NOT NULL DEFAULT '',
    additional_notes TEXT NOT NULL DEFAULT '',
    spec_type TEXT NOT NULL DEFAULT 'epic',
    status TEXT NOT NULL DEFAULT 'draft',
    version INTEGER NOT NULL DEFAULT 1,
    previous_version_id TEXT,
    task_id TEXT,
    source_ids TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    approved_at TEXT,
    approved_by TEXT,
    team_id TEXT,
    tags TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS idx_specs_status ON specs(status);
CREATE INDEX IF NOT EXISTS idx_specs_task_id ON specs(task_id);
"#;

/// Trait for spec storage operations
pub trait SpecStore: Send + Sync {
    /// Initialize the store (create tables, etc.)
    fn init(&self) -> Result<()>;

    /// Generate a new unique spec ID (e.g., spec-a1b2)
    fn generate_id(&self) -> Result<String>;

    /// Add a new spec
    fn add(&self, spec: &Spec) -> Result<()>;

    /// Get a spec by ID
    fn get(&self, id: &str) -> Result<Spec>;

    /// Update an existing spec
    fn update(&self, spec: &Spec) -> Result<()>;

    /// Delete a spec
    fn delete(&self, id: &str) -> Result<()>;

    /// List specs with optional status filter
    fn list(&self, status: Option<SpecStatus>) -> Result<Vec<Spec>>;

    /// List approved specs
    fn list_approved(&self) -> Result<Vec<Spec>>;

    /// Get specs for a specific task
    fn get_for_task(&self, task_id: &str) -> Result<Vec<Spec>>;

    /// Get all versions of a spec (by previous_version_id chain)
    fn get_versions(&self, spec_id: &str) -> Result<Vec<Spec>>;

    /// Search specs by title or summary
    fn search(&self, query: &str) -> Result<Vec<Spec>>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based spec store
pub struct SqliteSpecStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSpecStore {
    /// Open or create a SQLite spec store
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

    fn parse_json_array(s: &str) -> Vec<String> {
        if s.is_empty() || s == "[]" {
            return Vec::new();
        }
        serde_json::from_str(s).unwrap_or_default()
    }

    fn array_to_json(arr: &[String]) -> String {
        if arr.is_empty() {
            "[]".to_string()
        } else {
            serde_json::to_string(arr).unwrap_or_else(|_| "[]".to_string())
        }
    }

    /// Generate a hash-based ID like spec-a1b2
    fn generate_hash_id(&self) -> Result<String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        Utc::now().timestamp_nanos_opt().hash(&mut hasher);
        std::process::id().hash(&mut hasher);

        let hash = hasher.finish();
        let chars: Vec<char> = format!("{hash:016x}").chars().collect();

        let conn = crate::shared_db::lock_connection(&self.conn)?;
        for len in 4..=8 {
            let id = format!("spec-{}", chars[..len].iter().collect::<String>());
            let exists: bool = conn
                .query_row("SELECT 1 FROM specs WHERE id = ?", params![&id], |_| {
                    Ok(true)
                })
                .optional()?
                .unwrap_or(false);

            if !exists {
                return Ok(id);
            }
        }

        // Fallback to full hash
        Ok(format!("spec-{}", &chars[..12].iter().collect::<String>()))
    }

    fn spec_from_row(row: &rusqlite::Row) -> rusqlite::Result<Spec> {
        let scope_str: String = row.get(1)?;
        let spec_type_str: String = row.get(12)?;
        let status_str: String = row.get(13)?;

        Ok(Spec {
            id: row.get(0)?,
            scope: scope_str.parse().unwrap_or(Scope::Project),
            title: row.get(2)?,
            summary: row.get::<_, String>(3)?,
            goals: Self::parse_json_array(&row.get::<_, String>(4)?),
            in_scope: Self::parse_json_array(&row.get::<_, String>(5)?),
            out_of_scope: Self::parse_json_array(&row.get::<_, String>(6)?),
            users: Self::parse_json_array(&row.get::<_, String>(7)?),
            technical_requirements: Self::parse_json_array(&row.get::<_, String>(8)?),
            acceptance_criteria: Self::parse_json_array(&row.get::<_, String>(9)?),
            design_notes: row.get::<_, String>(10)?,
            additional_notes: row.get::<_, String>(11)?,
            spec_type: spec_type_str.parse().unwrap_or(SpecType::Epic),
            status: status_str.parse().unwrap_or(SpecStatus::Draft),
            version: row.get::<_, i32>(14)? as u32,
            previous_version_id: row.get::<_, Option<String>>(15)?,
            task_id: row.get::<_, Option<String>>(16)?,
            source_ids: Self::parse_json_array(&row.get::<_, String>(17)?),
            created_at: Self::parse_datetime(&row.get::<_, String>(18)?).unwrap_or_else(Utc::now),
            updated_at: Self::parse_datetime(&row.get::<_, String>(19)?).unwrap_or_else(Utc::now),
            approved_at: row
                .get::<_, Option<String>>(20)?
                .and_then(|s| Self::parse_datetime(&s)),
            approved_by: row.get::<_, Option<String>>(21)?,
            team_id: row.get::<_, Option<String>>(22)?,
            tags: Self::parse_json_array(&row.get::<_, String>(23)?),
        })
    }
}

impl SpecStore for SqliteSpecStore {
    fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(SPEC_SCHEMA)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        self.generate_hash_id()
    }

    fn add(&self, spec: &Spec) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO specs (
                id, scope, title, summary, goals, in_scope, out_of_scope, users,
                technical_requirements, acceptance_criteria, design_notes, additional_notes,
                spec_type, status, version, previous_version_id, task_id, source_ids,
                created_at, updated_at, approved_at, approved_by, team_id, tags
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                ?19, ?20, ?21, ?22, ?23, ?24
            )",
            params![
                spec.id,
                spec.scope.to_string(),
                spec.title,
                spec.summary,
                Self::array_to_json(&spec.goals),
                Self::array_to_json(&spec.in_scope),
                Self::array_to_json(&spec.out_of_scope),
                Self::array_to_json(&spec.users),
                Self::array_to_json(&spec.technical_requirements),
                Self::array_to_json(&spec.acceptance_criteria),
                spec.design_notes,
                spec.additional_notes,
                spec.spec_type.to_string(),
                spec.status.to_string(),
                spec.version as i32,
                spec.previous_version_id,
                spec.task_id,
                Self::array_to_json(&spec.source_ids),
                spec.created_at.to_rfc3339(),
                spec.updated_at.to_rfc3339(),
                spec.approved_at.map(|t| t.to_rfc3339()),
                spec.approved_by,
                spec.team_id,
                Self::array_to_json(&spec.tags),
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Spec> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT id, scope, title, summary, goals, in_scope, out_of_scope, users,
             technical_requirements, acceptance_criteria, design_notes, additional_notes,
             spec_type, status, version, previous_version_id, task_id, source_ids,
             created_at, updated_at, approved_at, approved_by, team_id, tags
             FROM specs WHERE id = ?",
            params![id],
            Self::spec_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("spec not found: {id}")))
    }

    fn update(&self, spec: &Spec) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE specs SET
                scope = ?1, title = ?2, summary = ?3, goals = ?4, in_scope = ?5,
                out_of_scope = ?6, users = ?7, technical_requirements = ?8,
                acceptance_criteria = ?9, design_notes = ?10, additional_notes = ?11,
                spec_type = ?12, status = ?13, version = ?14, previous_version_id = ?15,
                task_id = ?16, source_ids = ?17, updated_at = ?18, approved_at = ?19,
                approved_by = ?20, team_id = ?21, tags = ?22
             WHERE id = ?23",
            params![
                spec.scope.to_string(),
                spec.title,
                spec.summary,
                Self::array_to_json(&spec.goals),
                Self::array_to_json(&spec.in_scope),
                Self::array_to_json(&spec.out_of_scope),
                Self::array_to_json(&spec.users),
                Self::array_to_json(&spec.technical_requirements),
                Self::array_to_json(&spec.acceptance_criteria),
                spec.design_notes,
                spec.additional_notes,
                spec.spec_type.to_string(),
                spec.status.to_string(),
                spec.version as i32,
                spec.previous_version_id,
                spec.task_id,
                Self::array_to_json(&spec.source_ids),
                Utc::now().to_rfc3339(),
                spec.approved_at.map(|t| t.to_rfc3339()),
                spec.approved_by,
                spec.team_id,
                Self::array_to_json(&spec.tags),
                spec.id,
            ],
        )?;
        if rows == 0 {
            return Err(StoreError::NotFound(format!("spec not found: {}", spec.id)));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute("DELETE FROM specs WHERE id = ?", params![id])?;
        if rows == 0 {
            return Err(StoreError::NotFound(format!("spec not found: {id}")));
        }
        Ok(())
    }

    fn list(&self, status: Option<SpecStatus>) -> Result<Vec<Spec>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let (sql, params): (&str, Vec<String>) = match status {
            Some(s) => (
                "SELECT id, scope, title, summary, goals, in_scope, out_of_scope, users,
                 technical_requirements, acceptance_criteria, design_notes, additional_notes,
                 spec_type, status, version, previous_version_id, task_id, source_ids,
                 created_at, updated_at, approved_at, approved_by, team_id, tags
                 FROM specs WHERE status = ? ORDER BY updated_at DESC",
                vec![s.to_string()],
            ),
            None => (
                "SELECT id, scope, title, summary, goals, in_scope, out_of_scope, users,
                 technical_requirements, acceptance_criteria, design_notes, additional_notes,
                 spec_type, status, version, previous_version_id, task_id, source_ids,
                 created_at, updated_at, approved_at, approved_by, team_id, tags
                 FROM specs ORDER BY updated_at DESC",
                vec![],
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let specs = if params.is_empty() {
            stmt.query_map([], Self::spec_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![params[0]], Self::spec_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(specs)
    }

    fn list_approved(&self) -> Result<Vec<Spec>> {
        self.list(Some(SpecStatus::Approved))
    }

    fn get_for_task(&self, task_id: &str) -> Result<Vec<Spec>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, scope, title, summary, goals, in_scope, out_of_scope, users,
             technical_requirements, acceptance_criteria, design_notes, additional_notes,
             spec_type, status, version, previous_version_id, task_id, source_ids,
             created_at, updated_at, approved_at, approved_by, team_id, tags
             FROM specs WHERE task_id = ? ORDER BY version DESC",
        )?;

        let specs = stmt
            .query_map(params![task_id], Self::spec_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(specs)
    }

    fn get_versions(&self, spec_id: &str) -> Result<Vec<Spec>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        // First, find the root by traversing backwards through previous_version_id
        let mut current_id = spec_id.to_string();
        loop {
            let prev_id: Option<String> = conn
                .query_row(
                    "SELECT previous_version_id FROM specs WHERE id = ?",
                    params![&current_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();

            match prev_id {
                Some(pid) => current_id = pid,
                None => break,
            }
        }

        // current_id is now the root (earliest version)
        // Traverse forward by finding specs where previous_version_id = current
        let mut versions = Vec::new();
        loop {
            let spec: Option<Spec> = conn
                .query_row(
                    "SELECT id, scope, title, summary, goals, in_scope, out_of_scope, users,
                     technical_requirements, acceptance_criteria, design_notes, additional_notes,
                     spec_type, status, version, previous_version_id, task_id, source_ids,
                     created_at, updated_at, approved_at, approved_by, team_id, tags
                     FROM specs WHERE id = ?",
                    params![&current_id],
                    Self::spec_from_row,
                )
                .optional()?;

            match spec {
                Some(s) => {
                    let spec_id = s.id.clone();
                    versions.push(s);

                    // Find the next version in the chain
                    let next_id: Option<String> = conn
                        .query_row(
                            "SELECT id FROM specs WHERE previous_version_id = ?",
                            params![&spec_id],
                            |row| row.get(0),
                        )
                        .optional()?;

                    match next_id {
                        Some(nid) => current_id = nid,
                        None => break,
                    }
                }
                None => break,
            }
        }

        Ok(versions)
    }

    fn search(&self, query: &str) -> Result<Vec<Spec>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let pattern = format!("%{query}%");

        let mut stmt = conn.prepare_cached(
            "SELECT id, scope, title, summary, goals, in_scope, out_of_scope, users,
             technical_requirements, acceptance_criteria, design_notes, additional_notes,
             spec_type, status, version, previous_version_id, task_id, source_ids,
             created_at, updated_at, approved_at, approved_by, team_id, tags
             FROM specs
             WHERE title LIKE ?1 OR summary LIKE ?1 OR tags LIKE ?1 OR design_notes LIKE ?1
             ORDER BY updated_at DESC",
        )?;

        let specs = stmt
            .query_map(params![&pattern], Self::spec_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(specs)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::spec_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqliteSpecStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteSpecStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    #[test]
    fn test_spec_crud() {
        let (_temp, store) = create_test_store();

        // Create spec
        let id = store.generate_id().unwrap();
        let mut spec = Spec::new(id.clone(), "Test Spec".to_string());
        spec.summary = "A test spec description".to_string();
        spec.spec_type = SpecType::Epic;
        spec.goals = vec!["Goal 1".to_string(), "Goal 2".to_string()];
        spec.tags = vec!["test".to_string()];
        store.add(&spec).unwrap();

        // Get spec
        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.title, "Test Spec");
        assert_eq!(retrieved.summary, "A test spec description");
        assert_eq!(retrieved.spec_type, SpecType::Epic);
        assert_eq!(retrieved.goals, vec!["Goal 1", "Goal 2"]);
        assert_eq!(retrieved.tags, vec!["test"]);

        // Update spec
        spec.summary = "Updated description".to_string();
        spec.status = SpecStatus::UnderReview;
        store.update(&spec).unwrap();

        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.summary, "Updated description");
        assert_eq!(retrieved.status, SpecStatus::UnderReview);

        // List specs
        let all_specs = store.list(None).unwrap();
        assert_eq!(all_specs.len(), 1);

        let under_review = store.list(Some(SpecStatus::UnderReview)).unwrap();
        assert_eq!(under_review.len(), 1);

        let approved = store.list_approved().unwrap();
        assert_eq!(approved.len(), 0);

        // Delete spec
        store.delete(&id).unwrap();
        assert!(store.get(&id).is_err());
    }

    #[test]
    fn test_spec_search() {
        let (_temp, store) = create_test_store();

        // Create specs
        let spec1 = Spec {
            id: store.generate_id().unwrap(),
            title: "Authentication System".to_string(),
            summary: "User login and session management".to_string(),
            tags: vec!["security".to_string(), "auth".to_string()],
            ..Default::default()
        };
        let spec2 = Spec {
            id: store.generate_id().unwrap(),
            title: "Database Migration".to_string(),
            summary: "PostgreSQL to SQLite migration".to_string(),
            tags: vec!["database".to_string()],
            ..Default::default()
        };
        store.add(&spec1).unwrap();
        store.add(&spec2).unwrap();

        // Search by title
        let results = store.search("Authentication").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Authentication System");

        // Search by summary
        let results = store.search("PostgreSQL").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Database Migration");

        // Search by tag
        let results = store.search("security").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_spec_task_association() {
        let (_temp, store) = create_test_store();

        // Create specs with task association
        let spec1 = Spec {
            id: store.generate_id().unwrap(),
            title: "Epic Spec".to_string(),
            task_id: Some("task-001".to_string()),
            ..Default::default()
        };
        let spec2 = Spec {
            id: store.generate_id().unwrap(),
            title: "Another Spec".to_string(),
            task_id: Some("task-001".to_string()),
            ..Default::default()
        };
        let spec3 = Spec {
            id: store.generate_id().unwrap(),
            title: "Unrelated Spec".to_string(),
            task_id: Some("task-002".to_string()),
            ..Default::default()
        };
        store.add(&spec1).unwrap();
        store.add(&spec2).unwrap();
        store.add(&spec3).unwrap();

        // Get specs for task
        let task_specs = store.get_for_task("task-001").unwrap();
        assert_eq!(task_specs.len(), 2);

        let task_specs = store.get_for_task("task-002").unwrap();
        assert_eq!(task_specs.len(), 1);
        assert_eq!(task_specs[0].title, "Unrelated Spec");
    }

    #[test]
    fn test_spec_versions() {
        let (_temp, store) = create_test_store();

        // Create version chain: v1 -> v2 -> v3
        let id1 = store.generate_id().unwrap();
        let id2 = store.generate_id().unwrap();
        let id3 = store.generate_id().unwrap();

        let spec1 = Spec {
            id: id1.clone(),
            title: "Spec v1".to_string(),
            version: 1,
            previous_version_id: None,
            ..Default::default()
        };
        let spec2 = Spec {
            id: id2.clone(),
            title: "Spec v2".to_string(),
            version: 2,
            previous_version_id: Some(id1.clone()),
            ..Default::default()
        };
        let spec3 = Spec {
            id: id3.clone(),
            title: "Spec v3".to_string(),
            version: 3,
            previous_version_id: Some(id2.clone()),
            ..Default::default()
        };
        store.add(&spec1).unwrap();
        store.add(&spec2).unwrap();
        store.add(&spec3).unwrap();

        // Get versions from any point in chain
        let versions = store.get_versions(&id1).unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[2].version, 3);

        let versions = store.get_versions(&id3).unwrap();
        assert_eq!(versions.len(), 3);
    }

    #[test]
    fn test_spec_approved() {
        let (_temp, store) = create_test_store();

        let mut spec1 = Spec {
            id: store.generate_id().unwrap(),
            title: "Draft Spec".to_string(),
            status: SpecStatus::Draft,
            ..Default::default()
        };
        let spec2 = Spec {
            id: store.generate_id().unwrap(),
            title: "Approved Spec".to_string(),
            status: SpecStatus::Approved,
            approved_at: Some(Utc::now()),
            approved_by: Some("user-123".to_string()),
            ..Default::default()
        };
        store.add(&spec1).unwrap();
        store.add(&spec2).unwrap();

        // List approved only
        let approved = store.list_approved().unwrap();
        assert_eq!(approved.len(), 1);
        assert_eq!(approved[0].title, "Approved Spec");
        assert!(approved[0].approved_at.is_some());
        assert_eq!(approved[0].approved_by, Some("user-123".to_string()));

        // Approve the draft
        spec1.status = SpecStatus::Approved;
        spec1.approved_at = Some(Utc::now());
        store.update(&spec1).unwrap();

        let approved = store.list_approved().unwrap();
        assert_eq!(approved.len(), 2);
    }

    #[test]
    fn test_spec_id_format() {
        let (_temp, store) = create_test_store();

        let id = store.generate_id().unwrap();
        assert!(id.starts_with("spec-"));
        assert!(id.len() >= 9); // "spec-" + at least 4 chars
    }
}
