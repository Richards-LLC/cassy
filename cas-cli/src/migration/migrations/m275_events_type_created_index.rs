//! Index bounded event-type history in the explicit migration phase, never
//! while a runtime reader opens a populated store (cas-4fe2).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 275,
    name: "events_type_created_index",
    subsystem: Subsystem::Events,
    description: "Index event-type time windows for bounded supervisor retirement history",
    up: &[
        "CREATE INDEX IF NOT EXISTS idx_events_type_created ON events(event_type, created_at DESC)",
    ],
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_events_type_created'",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{EventStore, SqliteEventStore};
    use rusqlite::Connection;

    fn detected(conn: &Connection) -> i64 {
        conn.query_row(MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn history_index_is_idempotent_and_preserves_populated_history() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteEventStore::open(root.path()).unwrap();
        let conn = Connection::open(root.path().join("cas.db")).unwrap();
        assert_eq!(
            detected(&conn),
            0,
            "ordinary store open must not build a history index"
        );
        for i in 0..1000 {
            store
                .record(&cas_types::Event::new(
                    cas_types::EventType::AgentShutdown,
                    cas_types::EventEntityType::Agent,
                    format!("old-{i}"),
                    "retirement",
                ))
                .unwrap();
        }
        for _ in 0..2 {
            for sql in MIGRATION.up {
                conn.execute_batch(sql).unwrap();
            }
            assert_eq!(detected(&conn), 1);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1000
            );
        }
        let columns = conn
            .prepare("SELECT name FROM pragma_index_info('idx_events_type_created') ORDER BY seqno")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(columns, ["event_type", "created_at"]);
    }

    #[test]
    fn history_index_fresh_bootstrap_and_recorded_drop_reconcile() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            crate::store::init_cas_dir(&project).unwrap();
            let cas_dir = project.join(".cas");
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            assert_eq!(
                detected(&conn),
                1,
                "fresh init applies the explicit migration"
            );
            assert!(
                crate::migration::detector::detect_applied_migrations(&cas_dir)
                    .unwrap()
                    .contains(&275)
            );
            let store = SqliteEventStore::open(&cas_dir).unwrap();
            store
                .record(&cas_types::Event::new(
                    cas_types::EventType::AgentShutdown,
                    cas_types::EventEntityType::Agent,
                    "preserved",
                    "preserve on reconciliation",
                ))
                .unwrap();
            conn.execute_batch("DROP INDEX idx_events_type_created")
                .unwrap();
            drop(store);
            let _reader = SqliteEventStore::open(&cas_dir).unwrap();
            assert_eq!(
                detected(&conn),
                0,
                "hot readers must defer index rebuild to migration"
            );
            let result = crate::migration::run_migrations(&cas_dir, false).unwrap();
            assert_eq!(detected(&conn), 1);
            assert!(
                result
                    .reconciliations
                    .iter()
                    .any(|line| line.contains("275")),
                "{result:?}"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE entity_id='preserved'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            conn.execute("DELETE FROM cas_migrations", []).unwrap();
            crate::migration::bootstrap_migrations(&cas_dir).unwrap();
            crate::migration::run_migrations(&cas_dir, false).unwrap();
            assert_eq!(detected(&conn), 1);
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM cas_migrations WHERE id=275",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        });
    }
}
