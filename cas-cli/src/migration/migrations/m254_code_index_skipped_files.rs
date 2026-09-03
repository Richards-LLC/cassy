//! Migration: record undecodable source files as skipped, not failed (cas-bd9df).
//!
//! GH #698: a UTF-16 file counted as an index failure forever, because
//! `failed_files` is derived from `eligible - indexed` and no rerun could
//! change either number. Skipped files are excluded from the eligible
//! denominator instead, and are persisted separately so `cas doctor` can name
//! them without resurrecting the warning.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 254,
    name: "code_index_skipped_files",
    subsystem: Subsystem::Code,
    description: "Track undecodable source files as skipped-with-reason rather than as permanent index failures (cas-bd9df)",
    up: &[
        "ALTER TABLE code_index_state ADD COLUMN skipped_files INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE code_index_state ADD COLUMN skipped_detail TEXT",
    ],
    detect: Some(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('code_index_state') WHERE name = 'skipped_files'",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_skipped_columns_and_preserves_existing_scan_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE code_index_state (
                 repository TEXT PRIMARY KEY,
                 eligible_files INTEGER NOT NULL DEFAULT 0,
                 indexed_files INTEGER NOT NULL DEFAULT 0,
                 failed_files INTEGER NOT NULL DEFAULT 0,
                 last_head TEXT,
                 last_scan_at TEXT NOT NULL,
                 last_error TEXT
             );
             INSERT INTO code_index_state
                 (repository, eligible_files, indexed_files, failed_files, last_scan_at)
             VALUES ('gabber-studio', 2355, 2350, 5, '2026-09-03T19:17:00Z')",
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
        // The reporter's row survives untouched, defaulting to "nothing
        // skipped" until the next scan reclassifies its undecodable files.
        let row: (i64, i64, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT eligible_files, indexed_files, failed_files, skipped_files, skipped_detail
                 FROM code_index_state WHERE repository = 'gabber-studio'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row, (2355, 2350, 5, 0, None));
    }
}
