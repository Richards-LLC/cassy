//! cas-72d3: durable request identity and reconciliation state for Slack claims.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 278,
    name: "external_work_request_journal",
    subsystem: Subsystem::Agents,
    description: "Persist immutable external-work authority requests and retry state (cas-72d3)",
    up: crate::coordination::external_work::EXTERNAL_WORK_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
        EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='external_work_operations')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_external_work_due')
        THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn empty_root_init_runs_all_migrations_through_m278() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("empty-root");
            std::fs::create_dir_all(&project).unwrap();

            let cas_dir = crate::store::init_cas_dir(&project)
                .expect("fresh root should apply every registered migration");
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            let applied: i64 = conn
                .query_row("SELECT COUNT(*) FROM cas_migrations", [], |row| row.get(0))
                .unwrap();
            let latest: i64 = conn
                .query_row("SELECT MAX(id) FROM cas_migrations", [], |row| row.get(0))
                .unwrap();

            assert_eq!(applied, crate::migration::MIGRATIONS.len() as i64);
            assert_eq!(latest, crate::migration::MIGRATIONS.last().unwrap().id as i64);
            assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        });
    }

    #[test]
    fn journal_migration_recreates_the_schema_and_preserves_immutable_requests() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        for statement in MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'external_work_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0,
            "migration DDL must stay replay-safe; immutable triggers are store-initialized"
        );
        drop(conn);

        let _journal = crate::coordination::external_work::SqliteRequestJournal::open(temp.path())
            .expect("store initialization installs the immutable identity triggers");
        let conn = Connection::open(&db_path).unwrap();
        assert!(conn.execute(
            "INSERT INTO external_work_operations(request_id,kind,input_digest,input_json,stage,created_at,updated_at)
             VALUES ('id','claim','digest','{}','prepared','now','now')", []
        ).is_ok());
        assert!(
            conn.execute(
                "UPDATE external_work_operations SET input_json='{} changed' WHERE request_id='id'",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE external_work_operations SET wire_json='{}' WHERE request_id='id'",
                []
            )
            .is_ok()
        );
        assert!(conn.execute(
            r#"UPDATE external_work_operations SET wire_json='{"request_id":"changed"}' WHERE request_id='id'"#, []
        ).is_err());
    }
}
