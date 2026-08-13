//! SQLite storage backend for entities and relationships (knowledge graph)

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::StoreError;
use crate::{EntityStore, Result};
use cas_types::{Entity, EntityMention, EntityType, RelationType, Relationship};

/// SQLite DDL for the entity-graph tables (`entities`, `relationships`,
/// `entity_mentions`) and their indexes.
///
/// Re-exported via `cas_store::ENTITY_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base tables before applying ALTER migrations.
/// See cas-bdb9 / EPIC cas-9fdb.
pub const ENTITY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS entities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    aliases TEXT,
    description TEXT,
    created TEXT NOT NULL,
    updated TEXT NOT NULL,
    mention_count INTEGER NOT NULL DEFAULT 1,
    confidence REAL NOT NULL DEFAULT 0.8,
    archived INTEGER NOT NULL DEFAULT 0,
    metadata TEXT
);

CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(type);
CREATE INDEX IF NOT EXISTS idx_entities_archived ON entities(archived);

CREATE TABLE IF NOT EXISTS relationships (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    type TEXT NOT NULL,
    created TEXT NOT NULL,
    valid_from TEXT,
    valid_until TEXT,
    weight REAL NOT NULL DEFAULT 1.0,
    observation_count INTEGER NOT NULL DEFAULT 1,
    description TEXT,
    source_entries TEXT,
    FOREIGN KEY (source_id) REFERENCES entities(id) ON DELETE CASCADE,
    FOREIGN KEY (target_id) REFERENCES entities(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_relationships_source ON relationships(source_id);
CREATE INDEX IF NOT EXISTS idx_relationships_target ON relationships(target_id);
CREATE INDEX IF NOT EXISTS idx_relationships_type ON relationships(type);
CREATE UNIQUE INDEX IF NOT EXISTS idx_relationships_unique ON relationships(source_id, target_id, type);

CREATE TABLE IF NOT EXISTS entity_mentions (
    entity_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    position INTEGER,
    matched_text TEXT,
    confidence REAL NOT NULL DEFAULT 0.8,
    created TEXT NOT NULL,
    PRIMARY KEY (entity_id, entry_id),
    FOREIGN KEY (entity_id) REFERENCES entities(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_mentions_entity ON entity_mentions(entity_id);
CREATE INDEX IF NOT EXISTS idx_mentions_entry ON entity_mentions(entry_id);
"#;

/// SQLite-based entity store for knowledge graph
pub struct SqliteEntityStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEntityStore {
    /// Open or create a SQLite entity store
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

    fn parse_aliases(s: Option<String>) -> Vec<String> {
        s.map(|aliases| {
            if aliases.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&aliases).unwrap_or_default()
            }
        })
        .unwrap_or_default()
    }

    fn aliases_to_string(aliases: &[String]) -> String {
        if aliases.is_empty() {
            String::new()
        } else {
            serde_json::to_string(aliases).unwrap_or_default()
        }
    }

    fn parse_metadata(s: Option<String>) -> HashMap<String, String> {
        s.map(|meta| {
            if meta.is_empty() {
                HashMap::new()
            } else {
                serde_json::from_str(&meta).unwrap_or_default()
            }
        })
        .unwrap_or_default()
    }

    fn metadata_to_string(metadata: &HashMap<String, String>) -> String {
        if metadata.is_empty() {
            String::new()
        } else {
            serde_json::to_string(metadata).unwrap_or_default()
        }
    }

    fn parse_source_entries(s: Option<String>) -> Vec<String> {
        s.map(|entries| {
            if entries.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&entries).unwrap_or_default()
            }
        })
        .unwrap_or_default()
    }

    fn source_entries_to_string(entries: &[String]) -> String {
        if entries.is_empty() {
            String::new()
        } else {
            serde_json::to_string(entries).unwrap_or_default()
        }
    }

    fn row_to_entity(row: &rusqlite::Row) -> rusqlite::Result<Entity> {
        Ok(Entity {
            id: row.get(0)?,
            name: row.get(1)?,
            entity_type: row
                .get::<_, String>(2)?
                .parse()
                .unwrap_or(EntityType::Concept),
            aliases: Self::parse_aliases(row.get(3)?),
            description: row.get(4)?,
            created: Self::parse_datetime(&row.get::<_, String>(5)?).unwrap_or_else(Utc::now),
            updated: Self::parse_datetime(&row.get::<_, String>(6)?).unwrap_or_else(Utc::now),
            mention_count: row.get(7)?,
            confidence: row.get(8)?,
            archived: row.get::<_, i32>(9)? != 0,
            metadata: Self::parse_metadata(row.get(10)?),
            summary: None,         // Loaded separately if needed
            summary_updated: None, // Loaded separately if needed
        })
    }

    fn row_to_relationship(row: &rusqlite::Row) -> rusqlite::Result<Relationship> {
        Ok(Relationship {
            id: row.get(0)?,
            source_id: row.get(1)?,
            target_id: row.get(2)?,
            relation_type: row
                .get::<_, String>(3)?
                .parse()
                .unwrap_or(RelationType::RelatedTo),
            created: Self::parse_datetime(&row.get::<_, String>(4)?).unwrap_or_else(Utc::now),
            valid_from: row
                .get::<_, Option<String>>(5)?
                .and_then(|s| Self::parse_datetime(&s)),
            valid_until: row
                .get::<_, Option<String>>(6)?
                .and_then(|s| Self::parse_datetime(&s)),
            weight: row.get(7)?,
            observation_count: row.get(8)?,
            description: row.get(9)?,
            source_entries: Self::parse_source_entries(row.get(10)?),
        })
    }

    fn row_to_mention(row: &rusqlite::Row) -> rusqlite::Result<EntityMention> {
        Ok(EntityMention {
            entity_id: row.get(0)?,
            entry_id: row.get(1)?,
            position: row.get::<_, Option<i32>>(2)?.map(|p| p as usize),
            matched_text: row.get(3)?,
            confidence: row.get(4)?,
            created: Self::parse_datetime(&row.get::<_, String>(5)?).unwrap_or_else(Utc::now),
        })
    }
}

impl EntityStore for SqliteEntityStore {
    fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(ENTITY_SCHEMA)?;
        Ok(())
    }

    fn generate_entity_id(&self) -> Result<String> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let next = crate::shared_db::next_sequence_val(&conn, "entity")?;
        Ok(format!("ent-{next:04}"))
    }

    fn add_entity(&self, entity: &Entity) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO entities (id, name, type, aliases, description, created, updated,
             mention_count, confidence, archived, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                entity.id,
                entity.name,
                entity.entity_type.to_string(),
                Self::aliases_to_string(&entity.aliases),
                entity.description,
                entity.created.to_rfc3339(),
                entity.updated.to_rfc3339(),
                entity.mention_count,
                entity.confidence,
                entity.archived as i32,
                Self::metadata_to_string(&entity.metadata),
            ],
        )?;
        Ok(())
    }

    fn get_entity(&self, id: &str) -> Result<Entity> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let entity = conn
            .query_row(
                "SELECT id, name, type, aliases, description, created, updated,
                 mention_count, confidence, archived, metadata
                 FROM entities WHERE id = ?",
                params![id],
                Self::row_to_entity,
            )
            .optional()?
            .ok_or_else(|| StoreError::EntityNotFound(id.to_string()))?;
        Ok(entity)
    }

    fn get_entity_by_name(
        &self,
        name: &str,
        entity_type: Option<EntityType>,
    ) -> Result<Option<Entity>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let name_lower = name.to_lowercase();

        // First try exact name match
        let query = match entity_type {
            Some(et) => {
                let mut stmt = conn.prepare_cached(
                    "SELECT id, name, type, aliases, description, created, updated,
                     mention_count, confidence, archived, metadata
                     FROM entities WHERE LOWER(name) = ? AND type = ? AND archived = 0",
                )?;
                stmt.query_row(params![&name_lower, et.to_string()], Self::row_to_entity)
                    .optional()?
            }
            None => {
                let mut stmt = conn.prepare_cached(
                    "SELECT id, name, type, aliases, description, created, updated,
                     mention_count, confidence, archived, metadata
                     FROM entities WHERE LOWER(name) = ? AND archived = 0",
                )?;
                stmt.query_row(params![&name_lower], Self::row_to_entity)
                    .optional()?
            }
        };

        if let Some(entity) = query {
            return Ok(Some(entity));
        }

        // Check aliases (search in JSON array)
        let type_filter = entity_type
            .map(|t| format!(" AND type = '{t}'"))
            .unwrap_or_default();

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT id, name, type, aliases, description, created, updated,
             mention_count, confidence, archived, metadata
             FROM entities WHERE archived = 0{type_filter} AND aliases LIKE ?"
        ))?;

        let like_pattern = format!("%\"{name_lower}%");
        let entities: Vec<Entity> = stmt
            .query_map(params![like_pattern], Self::row_to_entity)?
            .filter_map(|r| r.ok())
            .collect();

        // Check each entity's aliases for exact match
        for entity in entities {
            if entity.matches_name(name) {
                return Ok(Some(entity));
            }
        }

        Ok(None)
    }

    fn update_entity(&self, entity: &Entity) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE entities SET name = ?1, type = ?2, aliases = ?3, description = ?4,
             updated = ?5, mention_count = ?6, confidence = ?7, archived = ?8, metadata = ?9
             WHERE id = ?10",
            params![
                entity.name,
                entity.entity_type.to_string(),
                Self::aliases_to_string(&entity.aliases),
                entity.description,
                entity.updated.to_rfc3339(),
                entity.mention_count,
                entity.confidence,
                entity.archived as i32,
                Self::metadata_to_string(&entity.metadata),
                entity.id,
            ],
        )?;
        if rows == 0 {
            return Err(StoreError::EntityNotFound(entity.id.clone()));
        }
        Ok(())
    }

    fn delete_entity(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute("DELETE FROM entities WHERE id = ?", params![id])?;
        if rows == 0 {
            return Err(StoreError::EntityNotFound(id.to_string()));
        }
        Ok(())
    }

    fn list_entities(&self, entity_type: Option<EntityType>) -> Result<Vec<Entity>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let (sql, type_param) = match &entity_type {
            Some(et) => (
                "SELECT id, name, type, aliases, description, created, updated,
                 mention_count, confidence, archived, metadata
                 FROM entities WHERE type = ? AND archived = 0 ORDER BY mention_count DESC",
                Some(et.to_string()),
            ),
            None => (
                "SELECT id, name, type, aliases, description, created, updated,
                 mention_count, confidence, archived, metadata
                 FROM entities WHERE archived = 0 ORDER BY mention_count DESC",
                None,
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let entities: Vec<Entity> = if let Some(type_str) = type_param {
            stmt.query_map(params![type_str], Self::row_to_entity)?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            stmt.query_map([], Self::row_to_entity)?
                .filter_map(|r| r.ok())
                .collect()
        };

        Ok(entities)
    }

    fn search_entities(&self, query: &str, entity_type: Option<EntityType>) -> Result<Vec<Entity>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let search_pattern = format!("%{}%", query.to_lowercase());

        let (sql, type_param) = match &entity_type {
            Some(et) => (
                "SELECT id, name, type, aliases, description, created, updated,
                 mention_count, confidence, archived, metadata
                 FROM entities
                 WHERE (LOWER(name) LIKE ? OR LOWER(aliases) LIKE ? OR LOWER(description) LIKE ?)
                   AND type = ? AND archived = 0
                 ORDER BY mention_count DESC",
                Some(et.to_string()),
            ),
            None => (
                "SELECT id, name, type, aliases, description, created, updated,
                 mention_count, confidence, archived, metadata
                 FROM entities
                 WHERE (LOWER(name) LIKE ? OR LOWER(aliases) LIKE ? OR LOWER(description) LIKE ?)
                   AND archived = 0
                 ORDER BY mention_count DESC",
                None,
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let entities: Vec<Entity> = if let Some(type_str) = type_param {
            stmt.query_map(
                params![&search_pattern, &search_pattern, &search_pattern, type_str],
                Self::row_to_entity,
            )?
            .filter_map(|r| r.ok())
            .collect()
        } else {
            stmt.query_map(
                params![&search_pattern, &search_pattern, &search_pattern],
                Self::row_to_entity,
            )?
            .filter_map(|r| r.ok())
            .collect()
        };

        Ok(entities)
    }

    fn generate_relationship_id(&self) -> Result<String> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let next = crate::shared_db::next_sequence_val(&conn, "relationship")?;
        Ok(format!("rel-{next:04}"))
    }

    fn add_relationship(&self, relationship: &Relationship) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO relationships (id, source_id, target_id, type, created,
             valid_from, valid_until, weight, observation_count, description, source_entries)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                relationship.id,
                relationship.source_id,
                relationship.target_id,
                relationship.relation_type.to_string(),
                relationship.created.to_rfc3339(),
                relationship.valid_from.map(|t| t.to_rfc3339()),
                relationship.valid_until.map(|t| t.to_rfc3339()),
                relationship.weight,
                relationship.observation_count,
                relationship.description,
                Self::source_entries_to_string(&relationship.source_entries),
            ],
        )?;
        Ok(())
    }

    fn get_relationship(&self, id: &str) -> Result<Relationship> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rel = conn
            .query_row(
                "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
                 weight, observation_count, description, source_entries
                 FROM relationships WHERE id = ?",
                params![id],
                Self::row_to_relationship,
            )
            .optional()?
            .ok_or_else(|| StoreError::RelationshipNotFound(id.to_string()))?;
        Ok(rel)
    }

    fn get_relationship_between(
        &self,
        source_id: &str,
        target_id: &str,
        relation_type: Option<RelationType>,
    ) -> Result<Option<Relationship>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let rel = match relation_type {
            Some(rt) => conn
                .query_row(
                    "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
                     weight, observation_count, description, source_entries
                     FROM relationships WHERE source_id = ? AND target_id = ? AND type = ?",
                    params![source_id, target_id, rt.to_string()],
                    Self::row_to_relationship,
                )
                .optional()?,
            None => conn
                .query_row(
                    "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
                     weight, observation_count, description, source_entries
                     FROM relationships WHERE source_id = ? AND target_id = ?",
                    params![source_id, target_id],
                    Self::row_to_relationship,
                )
                .optional()?,
        };

        Ok(rel)
    }

    fn update_relationship(&self, relationship: &Relationship) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE relationships SET source_id = ?1, target_id = ?2, type = ?3,
             valid_from = ?4, valid_until = ?5, weight = ?6, observation_count = ?7,
             description = ?8, source_entries = ?9
             WHERE id = ?10",
            params![
                relationship.source_id,
                relationship.target_id,
                relationship.relation_type.to_string(),
                relationship.valid_from.map(|t| t.to_rfc3339()),
                relationship.valid_until.map(|t| t.to_rfc3339()),
                relationship.weight,
                relationship.observation_count,
                relationship.description,
                Self::source_entries_to_string(&relationship.source_entries),
                relationship.id,
            ],
        )?;
        if rows == 0 {
            return Err(StoreError::RelationshipNotFound(relationship.id.clone()));
        }
        Ok(())
    }

    fn delete_relationship(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute("DELETE FROM relationships WHERE id = ?", params![id])?;
        if rows == 0 {
            return Err(StoreError::RelationshipNotFound(id.to_string()));
        }
        Ok(())
    }

    fn list_relationships(&self, relation_type: Option<RelationType>) -> Result<Vec<Relationship>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let (sql, type_param) = match &relation_type {
            Some(rt) => (
                "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
                 weight, observation_count, description, source_entries
                 FROM relationships WHERE type = ? ORDER BY weight DESC",
                Some(rt.to_string()),
            ),
            None => (
                "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
                 weight, observation_count, description, source_entries
                 FROM relationships ORDER BY weight DESC",
                None,
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let rels: Vec<Relationship> = if let Some(type_str) = type_param {
            stmt.query_map(params![type_str], Self::row_to_relationship)?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            stmt.query_map([], Self::row_to_relationship)?
                .filter_map(|r| r.ok())
                .collect()
        };

        Ok(rels)
    }

    fn get_entity_relationships(&self, entity_id: &str) -> Result<Vec<Relationship>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
             weight, observation_count, description, source_entries
             FROM relationships WHERE source_id = ? OR target_id = ? ORDER BY weight DESC",
        )?;

        let rels = stmt
            .query_map(params![entity_id, entity_id], Self::row_to_relationship)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rels)
    }

    fn get_outgoing_relationships(&self, entity_id: &str) -> Result<Vec<Relationship>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
             weight, observation_count, description, source_entries
             FROM relationships WHERE source_id = ? ORDER BY weight DESC",
        )?;

        let rels = stmt
            .query_map(params![entity_id], Self::row_to_relationship)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rels)
    }

    fn get_incoming_relationships(&self, entity_id: &str) -> Result<Vec<Relationship>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, type, created, valid_from, valid_until,
             weight, observation_count, description, source_entries
             FROM relationships WHERE target_id = ? ORDER BY weight DESC",
        )?;

        let rels = stmt
            .query_map(params![entity_id], Self::row_to_relationship)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rels)
    }

    fn add_mention(&self, mention: &EntityMention) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO entity_mentions (entity_id, entry_id, position,
             matched_text, confidence, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                mention.entity_id,
                mention.entry_id,
                mention.position.map(|p| p as i32),
                mention.matched_text,
                mention.confidence,
                mention.created.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn get_entity_mentions(&self, entity_id: &str) -> Result<Vec<EntityMention>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT entity_id, entry_id, position, matched_text, confidence, created
             FROM entity_mentions WHERE entity_id = ? ORDER BY created DESC",
        )?;

        let mentions = stmt
            .query_map(params![entity_id], Self::row_to_mention)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(mentions)
    }

    fn get_entry_mentions(&self, entry_id: &str) -> Result<Vec<EntityMention>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT entity_id, entry_id, position, matched_text, confidence, created
             FROM entity_mentions WHERE entry_id = ? ORDER BY position",
        )?;

        let mentions = stmt
            .query_map(params![entry_id], Self::row_to_mention)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(mentions)
    }

    fn delete_entry_mentions(&self, entry_id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "DELETE FROM entity_mentions WHERE entry_id = ?",
            params![entry_id],
        )?;
        Ok(())
    }

    fn get_connected_entities(&self, entity_id: &str) -> Result<Vec<(Entity, Relationship)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        // Get all relationships involving this entity
        let mut stmt = conn.prepare_cached(
            "SELECT r.id, r.source_id, r.target_id, r.type, r.created, r.valid_from, r.valid_until,
                    r.weight, r.observation_count, r.description, r.source_entries,
                    e.id, e.name, e.type, e.aliases, e.description, e.created, e.updated,
                    e.mention_count, e.confidence, e.archived, e.metadata
             FROM relationships r
             JOIN entities e ON (
                 (r.source_id = ? AND r.target_id = e.id) OR
                 (r.target_id = ? AND r.source_id = e.id)
             )
             WHERE e.archived = 0
             ORDER BY r.weight DESC",
        )?;

        let results: Vec<(Entity, Relationship)> = stmt
            .query_map(params![entity_id, entity_id], |row| {
                let rel = Self::row_to_relationship(row)?;
                let entity = Entity {
                    id: row.get(11)?,
                    name: row.get(12)?,
                    entity_type: row
                        .get::<_, String>(13)?
                        .parse()
                        .unwrap_or(EntityType::Concept),
                    aliases: Self::parse_aliases(row.get(14)?),
                    description: row.get(15)?,
                    created: Self::parse_datetime(&row.get::<_, String>(16)?)
                        .unwrap_or_else(Utc::now),
                    updated: Self::parse_datetime(&row.get::<_, String>(17)?)
                        .unwrap_or_else(Utc::now),
                    mention_count: row.get(18)?,
                    confidence: row.get(19)?,
                    archived: row.get::<_, i32>(20)? != 0,
                    metadata: Self::parse_metadata(row.get(21)?),
                    summary: None,
                    summary_updated: None,
                };
                Ok((entity, rel))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    fn get_entity_entries(&self, entity_id: &str, limit: usize) -> Result<Vec<String>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT entry_id FROM entity_mentions
             WHERE entity_id = ? ORDER BY created DESC LIMIT ?",
        )?;

        let entries: Vec<String> = stmt
            .query_map(params![entity_id, limit as i64], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "entity_store_tests/tests.rs"]
mod tests;
