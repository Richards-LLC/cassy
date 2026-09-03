//! Migration: record both sides' revisions on a journaled conflict (cas-c32f).
//!
//! A conflict row that names only two timestamps cannot be audited once
//! revisions decide the winner — the operator needs to see the revisions that
//! actually settled it. Both columns are nullable: a conflict resolved on the
//! timestamp path (either side lacking a revision) legitimately has none.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 252,
    name: "sync_conflicts_add_revisions",
    subsystem: Subsystem::Tasks,
    description: "Record local and remote revisions on journaled sync conflicts (cas-c32f)",
    up: &[
        "ALTER TABLE sync_conflicts ADD COLUMN local_revision INTEGER",
        "ALTER TABLE sync_conflicts ADD COLUMN remote_revision INTEGER",
    ],
    detect: Some(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('sync_conflicts') WHERE name = 'local_revision'",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn journal_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE sync_conflicts (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 entity_type TEXT NOT NULL,
                 entity_id TEXT NOT NULL,
                 discarded_row_json TEXT NOT NULL,
                 winner_side TEXT NOT NULL,
                 strategy TEXT NOT NULL,
                 resolved_at TEXT NOT NULL
             )",
        )
        .unwrap();
    }

    #[test]
    fn migration_adds_nullable_revision_columns_to_the_journal() {
        let conn = Connection::open_in_memory().unwrap();
        journal_table(&conn);
        conn.execute(
            "INSERT INTO sync_conflicts (entity_type, entity_id, discarded_row_json, winner_side, strategy, resolved_at)
             VALUES ('task', 'cas-old', '{}', 'remote', 'timestamp_lww', '2026-09-01T00:00:00Z')",
            [],
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );

        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }

        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        // The pre-existing timestamp-resolved row survives with NULL revisions.
        let existing: (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT local_revision, remote_revision FROM sync_conflicts WHERE entity_id = 'cas-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(existing, (None, None));

        let revisions: (Option<i64>, Option<i64>) = conn
            .query_row(
                "INSERT INTO sync_conflicts
                     (entity_type, entity_id, discarded_row_json, winner_side, strategy, resolved_at, local_revision, remote_revision)
                 VALUES ('task', 'cas-new', '{}', 'remote', 'revision', '2026-09-03T00:00:00Z', 4, 7)
                 RETURNING local_revision, remote_revision",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(revisions, (Some(4), Some(7)));
    }
}
