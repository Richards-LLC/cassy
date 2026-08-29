//! Migration: Add provenance source IDs to skills.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 238,
    name: "skills_add_source_ids",
    subsystem: Subsystem::Skills,
    description: "Add source_ids JSON column to skills for provenance links",
    up: &["ALTER TABLE skills ADD COLUMN source_ids TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('skills') WHERE name = 'source_ids') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn adds_source_ids_without_changing_existing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO skills (id, name) VALUES ('skill-1', 'legacy');",
        )
        .unwrap();

        conn.execute(super::MIGRATION.up[0], []).unwrap();

        let source_ids: Option<String> = conn
            .query_row(
                "SELECT source_ids FROM skills WHERE id = 'skill-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(source_ids.is_none());
    }
}
