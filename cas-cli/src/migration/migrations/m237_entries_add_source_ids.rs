//! Migration: Add provenance source IDs to memory entries.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 237,
    name: "entries_add_source_ids",
    subsystem: Subsystem::Entries,
    description: "Add source_ids JSON column to entries for provenance links",
    up: &["ALTER TABLE entries ADD COLUMN source_ids TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('entries') WHERE name = 'source_ids') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn adds_source_ids_without_changing_existing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL DEFAULT 'learning',
                content TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO entries (id, content) VALUES ('entry-1', 'legacy');",
        )
        .unwrap();

        conn.execute(super::MIGRATION.up[0], []).unwrap();

        let source_ids: Option<String> = conn
            .query_row(
                "SELECT source_ids FROM entries WHERE id = 'entry-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(source_ids.is_none());
    }
}
