//! Migration: durable rule and skill history for reversible lifecycle changes.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 240,
    name: "rule_skill_versions",
    subsystem: Subsystem::Rules,
    description: "Create rule and skill version history for reversible updates and tombstone deletes (cas-30af)",
    up: cas_store::RULE_AND_SKILL_VERSIONS_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rule_versions')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'skill_versions')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_both_version_ledgers_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
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
            conn.query_row("SELECT COUNT(*) FROM pragma_table_info('rule_versions')", [], |row| row.get::<_, i64>(0)).unwrap(),
            9
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM pragma_table_info('skill_versions')", [], |row| row.get::<_, i64>(0)).unwrap(),
            10
        );
    }
}
