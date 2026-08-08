//! Migration: make knowledge-page provenance durable (cas-8c84).
//!
//! Existing pages can only be described honestly as local: before this
//! migration no cloud-pull attribution survived the write. In particular, the
//! global knowledge store has no project identity, so the backfill deliberately
//! leaves `origin_project_id` NULL instead of inventing one.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 226,
    name: "knowledge_pages_add_attribution",
    subsystem: Subsystem::Knowledge,
    description: "Add local/cloud-pull provenance to knowledge pages and backfill existing rows as local (cas-8c84)",
    up: cas_store::KNOWLEDGE_PAGE_ATTRIBUTION_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
             NOT EXISTS (
                 SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'knowledge_pages'
             )
             OR (
                 EXISTS (SELECT 1 FROM pragma_table_info('knowledge_pages') WHERE name = 'origin')
                 AND EXISTS (
                     SELECT 1 FROM pragma_table_info('knowledge_pages')
                     WHERE name = 'origin_project_id'
                 )
             )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn legacy_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE knowledge_pages (
                 row_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 id TEXT NOT NULL UNIQUE,
                 page_type TEXT NOT NULL,
                 title TEXT NOT NULL,
                 rel_path TEXT NOT NULL,
                 snippet TEXT NOT NULL DEFAULT '',
                 locked INTEGER NOT NULL DEFAULT 0,
                 sources_json TEXT NOT NULL DEFAULT '[]',
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 pending_embedding INTEGER NOT NULL DEFAULT 1
             );
             INSERT INTO knowledge_pages
                 (id, page_type, title, rel_path, created_at, updated_at)
             VALUES ('cas-kn001', 'architecture', 'Local', 'architecture/local.md',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .unwrap();
    }

    #[test]
    fn migration_backfills_existing_pages_as_local_without_inventing_project_identity() {
        let conn = Connection::open_in_memory().unwrap();
        legacy_table(&conn);

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let (origin, project_id): (String, Option<String>) = conn
            .query_row(
                "SELECT origin, origin_project_id FROM knowledge_pages WHERE id = 'cas-kn001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(origin, "local");
        assert_eq!(project_id, None);
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn detect_skips_absent_table_and_finds_legacy_shape() {
        let conn = Connection::open_in_memory().unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        legacy_table(&conn);
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
