use crate::error::StoreError;
use crate::sqlite::{ENTRIES_RULES_SCHEMA, SqliteRuleStore};
use crate::tracing::{DevTracer, TraceTimer};
use crate::version_store::{
    RULE_VERSIONS_SCHEMA_STATEMENTS, RuleVersion, default_changed_by, parse_datetime,
};
use crate::{Result, RuleStore};
use cas_types::{Rule, RuleStatus};
use chrono::Utc;
use rusqlite::{OptionalExtension, Transaction, params};

impl SqliteRuleStore {
    fn insert_version(
        tx: &Transaction<'_>,
        rule_id: &str,
        snapshot_json: &str,
        content: &str,
        status: RuleStatus,
        changed_by: &Option<String>,
        changed_at: &str,
        change_note: &str,
        operation: &str,
    ) -> Result<()> {
        let next_version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM rule_versions WHERE rule_id = ?1",
            params![rule_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO rule_versions
             (rule_id, version, snapshot_json, content, status, changed_by, changed_at, change_note, operation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                rule_id,
                next_version,
                snapshot_json,
                content,
                status.to_string(),
                changed_by,
                changed_at,
                change_note,
                operation,
            ],
        )?;
        Ok(())
    }

    fn update_recorded(
        &self,
        rule: &Rule,
        changed_by: Option<&str>,
        change_note: Option<&str>,
        operation: &str,
    ) -> Result<()> {
        let timer = TraceTimer::new();
        let previous = self.get(&rule.id)?;
        let snapshot_json = serde_json::to_string(&previous).map_err(|error| {
            StoreError::Parse(format!("failed to serialize rule history: {error}"))
        })?;
        let changed_by = changed_by
            .map(ToOwned::to_owned)
            .or_else(default_changed_by);
        let change_note = change_note.unwrap_or("update");
        let changed_at = Utc::now().to_rfc3339();

        let result = (|| -> Result<()> {
            let mut conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = conn.transaction()?;
            Self::insert_version(
                &tx,
                &rule.id,
                &snapshot_json,
                &previous.content,
                previous.status,
                &changed_by,
                &changed_at,
                change_note,
                operation,
            )?;
            let rows = tx.execute(
                "UPDATE rules SET source_ids = ?1, helpful_count = ?2, harmful_count = ?3,
                 tags = ?4, paths = ?5, content = ?6, status = ?7, last_accessed = ?8,
                 review_after = ?9, category = ?10, priority = ?11,
                 surface_count = ?12, scope = ?13, auto_approve_tools = ?14, auto_approve_paths = ?15, team_id = ?16, share = ?17
                 WHERE id = ?18",
                params![
                    Self::source_ids_to_string(&rule.source_ids),
                    rule.helpful_count,
                    rule.harmful_count,
                    Self::tags_to_string(&rule.tags),
                    rule.paths,
                    rule.content,
                    rule.status.to_string(),
                    rule.last_accessed.map(|t| t.to_rfc3339()),
                    rule.review_after.map(|t| t.to_rfc3339()),
                    rule.category.to_string(),
                    rule.priority,
                    rule.surface_count,
                    rule.scope.to_string(),
                    rule.auto_approve_tools.as_ref(),
                    rule.auto_approve_paths.as_ref(),
                    rule.team_id.as_ref(),
                    rule.share.as_ref().map(|s| s.to_string()),
                    rule.id,
                ],
            )?;
            if rows == 0 {
                return Err(StoreError::RuleNotFound(rule.id.clone()).into());
            }
            tx.commit()?;
            Ok(())
        })();

        if let Some(tracer) = DevTracer::get() {
            let (success, error) = match &result {
                Ok(_) => (true, None),
                Err(error) => (false, Some(error.to_string())),
            };
            let _ = tracer.record_store_op(
                "update_rule",
                "sqlite",
                &[rule.id.as_str()],
                if success { 1 } else { 0 },
                timer.elapsed_ms(),
                success,
                error.as_deref(),
            );
        }
        result
    }

    fn delete_recorded(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let previous = self.get(id)?;
        if previous.status == RuleStatus::Retired {
            return Ok(());
        }
        let mut retired = previous;
        retired.status = RuleStatus::Retired;
        self.update_recorded(
            &retired,
            changed_by,
            Some(change_note.unwrap_or("tombstone delete")),
            "delete",
        )
    }

    pub fn list_versions(&self, id: &str) -> Result<Vec<RuleVersion>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, rule_id, version, snapshot_json, content, status,
                    changed_by, changed_at, change_note, operation
             FROM rule_versions WHERE rule_id = ?1 ORDER BY version DESC",
        )?;
        let versions = stmt
            .query_map(params![id], |row| {
                Ok(RuleVersion {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    version: row.get(2)?,
                    snapshot_json: row.get(3)?,
                    content: row.get(4)?,
                    status: row
                        .get::<_, String>(5)?
                        .parse()
                        .unwrap_or(RuleStatus::Draft),
                    changed_by: row.get(6)?,
                    changed_at: parse_datetime(&row.get::<_, String>(7)?),
                    change_note: row.get(8)?,
                    operation: row.get(9)?,
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
                    "SELECT snapshot_json FROM rule_versions WHERE rule_id = ?1 AND version = ?2",
                    params![id, version],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::Parse(format!("rule version not found: {id} v{version}")))?,
            None => conn
                .query_row(
                    "SELECT snapshot_json FROM rule_versions WHERE rule_id = ?1 ORDER BY version DESC LIMIT 1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::Parse(format!("no rule history for: {id}")))?,
        };
        drop(conn);

        let restored: Rule = serde_json::from_str(&snapshot_json).map_err(|error| {
            StoreError::Parse(format!("invalid rule history for {id}: {error}"))
        })?;
        if restored.id != id {
            return Err(StoreError::Parse(format!(
                "rule history ID mismatch for: {id}"
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

impl RuleStore for SqliteRuleStore {
    fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(ENTRIES_RULES_SCHEMA)?;
        for statement in RULE_VERSIONS_SCHEMA_STATEMENTS {
            conn.execute(statement, [])?;
        }
        // NOTE: Column migrations are handled by `cas update --schema-only`
        // See cas-cli/src/migration/migrations.rs for migration definitions (IDs 51-56)
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        loop {
            let next = crate::shared_db::next_sequence_val(&conn, "rule")?;
            let id = format!("rule-{next:03}");
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM rules WHERE id = ?1)",
                params![&id],
                |row| row.get(0),
            )?;

            if !exists {
                return Ok(id);
            }

            let max_existing: i64 = conn.query_row(
                "SELECT COALESCE(
                    MAX(CASE
                        WHEN id GLOB 'rule-[0-9]*' THEN CAST(SUBSTR(id, 6) AS INTEGER)
                    END),
                    ?1
                 ) FROM rules",
                params![next],
                |row| row.get(0),
            )?;

            conn.execute(
                "INSERT INTO id_sequences (name, next_val) VALUES ('rule', ?1)
                 ON CONFLICT(name) DO UPDATE SET next_val =
                    CASE
                        WHEN next_val < excluded.next_val THEN excluded.next_val
                        ELSE next_val
                    END",
                params![max_existing],
            )?;
        }
    }

    fn add(&self, rule: &Rule) -> Result<()> {
        let timer = TraceTimer::new();
        let snapshot_json = serde_json::to_string(rule).map_err(|error| {
            StoreError::Parse(format!("failed to serialize rule history: {error}"))
        });
        let result = snapshot_json.and_then(|snapshot_json| {
            let changed_by = default_changed_by();
            let changed_at = Utc::now().to_rfc3339();
            let mut conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO rules (id, created, source_ids, helpful_count, harmful_count,
                 tags, paths, content, status, last_accessed, review_after,
                 category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    rule.id,
                    rule.created.to_rfc3339(),
                    Self::source_ids_to_string(&rule.source_ids),
                    rule.helpful_count,
                    rule.harmful_count,
                    Self::tags_to_string(&rule.tags),
                    rule.paths,
                    rule.content,
                    rule.status.to_string(),
                    rule.last_accessed.map(|t| t.to_rfc3339()),
                    rule.review_after.map(|t| t.to_rfc3339()),
                    rule.category.to_string(),
                    rule.priority,
                    rule.surface_count,
                    rule.scope.to_string(),
                    rule.auto_approve_tools.as_ref(),
                    rule.auto_approve_paths.as_ref(),
                    rule.team_id.as_ref(),
                    rule.share.as_ref().map(|s| s.to_string()),
                ],
            )?;
            Self::insert_version(
                &tx,
                &rule.id,
                &snapshot_json,
                &rule.content,
                rule.status,
                &changed_by,
                &changed_at,
                "create",
                "create",
            )?;
            tx.commit()?;
            Ok(())
        });

        // Record trace
        if let Some(tracer) = DevTracer::get() {
            let (success, error) = match &result {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            };
            let _ = tracer.record_store_op(
                "add_rule",
                "sqlite",
                &[rule.id.as_str()],
                if success { 1 } else { 0 },
                timer.elapsed_ms(),
                success,
                error.as_deref(),
            );
        }

        result?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Rule> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rule = conn
            .query_row(
                "SELECT id, created, source_ids, helpful_count, harmful_count,
                 tags, paths, content, status, last_accessed, review_after,
                 category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share
                 FROM rules WHERE id = ?",
                params![id],
                |row| {
                    Ok(Rule {
                        id: row.get(0)?,
                        scope: Self::parse_scope(row.get(14)?),
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
                        category: row
                            .get::<_, Option<String>>(11)?
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default(),
                        priority: row.get::<_, Option<u8>>(12)?.unwrap_or(2),
                        surface_count: row.get::<_, Option<i32>>(13)?.unwrap_or(0),
                        auto_approve_tools: row.get(15)?,
                        auto_approve_paths: row.get(16)?,
                        team_id: row.get(17)?,
                        share: row
                            .get::<_, Option<String>>(18)?
                            .as_deref()
                            .and_then(|s| s.parse().ok()),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::RuleNotFound(id.to_string()))?;
        Ok(rule)
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        self.update_recorded(rule, None, None, "update")
    }

    fn update_with_metadata(
        &self,
        rule: &Rule,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.update_recorded(rule, changed_by, change_note, "update")
    }

    fn increment_surface_count(&self, id: &str) -> Result<()> {
        let timer = TraceTimer::new();
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let result = conn.execute(
            "UPDATE rules
             SET surface_count = CASE
                 WHEN surface_count < 2147483647 THEN surface_count + 1
                 ELSE surface_count
             END
             WHERE id = ?1",
            params![id],
        );

        if let Some(tracer) = DevTracer::get() {
            let (success, error) = match &result {
                Ok(rows) => (*rows > 0, None),
                Err(e) => (false, Some(e.to_string())),
            };
            let _ = tracer.record_store_op(
                "increment_rule_surface_count",
                "sqlite",
                &[id],
                result.as_ref().copied().unwrap_or(0),
                timer.elapsed_ms(),
                success,
                error.as_deref(),
            );
        }

        let rows = result?;
        if rows == 0 {
            return Err(StoreError::RuleNotFound(id.to_string()));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let timer = TraceTimer::new();
        let result = self.delete_recorded(id, None, None);

        // Record trace
        if let Some(tracer) = DevTracer::get() {
            let (success, error) = match &result {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            };
            let _ = tracer.record_store_op(
                "delete_rule",
                "sqlite",
                &[id],
                if result.is_ok() { 1 } else { 0 },
                timer.elapsed_ms(),
                success,
                error.as_deref(),
            );
        }

        result
    }

    fn delete_with_metadata(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.delete_recorded(id, changed_by, change_note)
    }

    fn list_versions(&self, id: &str) -> Result<Vec<RuleVersion>> {
        SqliteRuleStore::list_versions(self, id)
    }

    fn restore_version(
        &self,
        id: &str,
        version: Option<i64>,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        SqliteRuleStore::restore_version(self, id, version, changed_by, change_note)
    }

    fn list(&self) -> Result<Vec<Rule>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, created, source_ids, helpful_count, harmful_count,
             tags, paths, content, status, last_accessed, review_after,
             category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share
             FROM rules ORDER BY priority ASC, created DESC",
        )?;

        let rules = stmt
            .query_map([], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    scope: Self::parse_scope(row.get(14)?),
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
                    category: row
                        .get::<_, Option<String>>(11)?
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    priority: row.get::<_, Option<u8>>(12)?.unwrap_or(2),
                    surface_count: row.get::<_, Option<i32>>(13)?.unwrap_or(0),
                    auto_approve_tools: row.get(15)?,
                    auto_approve_paths: row.get(16)?,
                    team_id: row.get(17)?,
                    share: row
                        .get::<_, Option<String>>(18)?
                        .as_deref()
                        .and_then(|s| s.parse().ok()),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rules)
    }

    fn list_proven(&self) -> Result<Vec<Rule>> {
        // Call the inherent method
        SqliteRuleStore::list_proven(self)
    }

    fn list_critical(&self) -> Result<Vec<Rule>> {
        // Call the inherent method
        SqliteRuleStore::list_critical(self)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}
