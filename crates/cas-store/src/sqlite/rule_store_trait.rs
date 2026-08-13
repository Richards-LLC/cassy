use crate::error::StoreError;
use crate::sqlite::{ENTRIES_RULES_SCHEMA, SqliteRuleStore};
use crate::tracing::{DevTracer, TraceTimer};
use crate::{Result, RuleStore};
use cas_types::{Rule, RuleStatus};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

impl RuleStore for SqliteRuleStore {
    fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(ENTRIES_RULES_SCHEMA)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let result = conn.execute(
            "INSERT INTO rules (id, created, source_ids, helpful_count, harmful_count,
             tags, paths, content, status, last_accessed, review_after, hook_command,
             category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
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
                rule.hook_command.as_ref(),
                rule.category.to_string(),
                rule.priority,
                rule.surface_count,
                rule.scope.to_string(),
                rule.auto_approve_tools.as_ref(),
                rule.auto_approve_paths.as_ref(),
                rule.team_id.as_ref(),
                rule.share.as_ref().map(|s| s.to_string()),
            ],
        );

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
                 tags, paths, content, status, last_accessed, review_after, hook_command,
                 category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share
                 FROM rules WHERE id = ?",
                params![id],
                |row| {
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
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::RuleNotFound(id.to_string()))?;
        Ok(rule)
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        let timer = TraceTimer::new();
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let result = conn.execute(
            "UPDATE rules SET source_ids = ?1, helpful_count = ?2, harmful_count = ?3,
             tags = ?4, paths = ?5, content = ?6, status = ?7, last_accessed = ?8,
             review_after = ?9, hook_command = ?10, category = ?11, priority = ?12,
             surface_count = ?13, scope = ?14, auto_approve_tools = ?15, auto_approve_paths = ?16, team_id = ?17, share = ?18
             WHERE id = ?19",
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
                rule.hook_command.as_ref(),
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
        );

        // Record trace
        if let Some(tracer) = DevTracer::get() {
            let (success, error) = match &result {
                Ok(rows) => (*rows > 0, None),
                Err(e) => (false, Some(e.to_string())),
            };
            let _ = tracer.record_store_op(
                "update_rule",
                "sqlite",
                &[rule.id.as_str()],
                result.as_ref().copied().unwrap_or(0),
                timer.elapsed_ms(),
                success,
                error.as_deref(),
            );
        }

        let rows = result?;
        if rows == 0 {
            return Err(StoreError::RuleNotFound(rule.id.clone()));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let timer = TraceTimer::new();
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let result = conn.execute("DELETE FROM rules WHERE id = ?", params![id]);

        // Record trace
        if let Some(tracer) = DevTracer::get() {
            let (success, error) = match &result {
                Ok(rows) => (*rows > 0, None),
                Err(e) => (false, Some(e.to_string())),
            };
            let _ = tracer.record_store_op(
                "delete_rule",
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

    fn list(&self) -> Result<Vec<Rule>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, created, source_ids, helpful_count, harmful_count,
             tags, paths, content, status, last_accessed, review_after, hook_command,
             category, priority, surface_count, scope, auto_approve_tools, auto_approve_paths, team_id, share
             FROM rules ORDER BY priority ASC, created DESC",
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
