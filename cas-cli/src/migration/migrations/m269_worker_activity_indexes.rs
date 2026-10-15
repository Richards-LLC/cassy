//! Build worker activity indexes in the explicit migration phase, never store open.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 269,
    name: "worker_activity_indexes",
    subsystem: Subsystem::Events,
    description: "Index per-worker activity without fleet-window eviction",
    up: &[
        "CREATE INDEX IF NOT EXISTS idx_events_session_created ON events(session_id, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_events_worker_push_activity ON events(entity_id, created_at DESC) WHERE event_type = 'worker_push_blocked'",
    ],
    detect: Some("SELECT CASE WHEN COUNT(*) = 2 THEN 1 ELSE 0 END FROM sqlite_master WHERE type = 'index' AND name IN ('idx_events_session_created', 'idx_events_worker_push_activity')"),
};

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{EventStore, SqliteEventStore};

    #[test]
    fn worker_activity_index_migration_preserves_populated_history_and_reconciles() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteEventStore::open(root.path()).unwrap();
        for i in 0..1000 {
            let event = cas_types::Event::new(cas_types::EventType::TaskNoteAdded,
                cas_types::EventEntityType::Task, "cas-history", &format!("note {i}"))
                .with_session("worker");
            store.record(&event).unwrap();
        }
        let conn = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
        let detected = || conn.query_row(MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(detected(), 0, "ordinary event-store open must not build these indexes");
        for _ in 0..2 {
            for sql in MIGRATION.up { conn.execute_batch(sql).unwrap(); }
            assert_eq!(detected(), 1);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(), 1000);
        }
        conn.execute_batch("DROP INDEX idx_events_worker_push_activity").unwrap();
        assert_eq!(detected(), 0, "partial application must remain pending");
        for sql in MIGRATION.up { conn.execute_batch(sql).unwrap(); }
        assert_eq!(detected(), 1);
        assert_eq!(store.latest_worker_activity("worker", "worker").unwrap().unwrap().summary, "note 999");
    }
}
