//! Migration: distinguish explicit, automatic, and judge retrieval outcomes.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 245,
    name: "retrieval_outcomes_add_attribution",
    subsystem: Subsystem::Events,
    description: "Tag retrieval outcomes by their feedback attribution (cas-8f93)",
    up: &["ALTER TABLE retrieval_outcomes ADD COLUMN attribution TEXT NOT NULL DEFAULT 'explicit'"],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'retrieval_outcomes')
            AND EXISTS (SELECT 1 FROM pragma_table_info('retrieval_outcomes')
                        WHERE name = 'attribution')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_attribution_and_preserves_existing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE retrieval_outcomes (
                id TEXT PRIMARY KEY,
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                outcome TEXT NOT NULL,
                actor_hash TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                correction_ref TEXT,
                created_at TEXT NOT NULL
             );
             INSERT INTO retrieval_outcomes
                 (id, query_id, result_id, outcome, actor_hash, session_hash, created_at)
             VALUES ('legacy', 'q1', 'entry-1', 'ignored', 'actor', 'session', 'now');",
        )
        .unwrap();

        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT attribution FROM retrieval_outcomes WHERE id = 'legacy'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "explicit"
        );
    }
}
