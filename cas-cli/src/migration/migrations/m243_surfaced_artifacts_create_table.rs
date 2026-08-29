//! Migration: durable per-session rule and skill injection records.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 243,
    name: "surfaced_artifacts_create_table",
    subsystem: Subsystem::Events,
    description: "Record injected rules and skills for session-outcome impact reports (cas-a9be)",
    up: cas_store::SURFACED_ARTIFACT_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'surfaced_artifacts')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_surfaced_artifacts_session')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_surfaced_artifacts_artifact')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_surface_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );

        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('surfaced_artifacts')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            6
        );
    }
}
