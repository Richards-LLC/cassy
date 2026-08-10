//! Migration: retain pull-side sync conflicts for operator review (cas-ab2f).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 231,
    name: "sync_conflicts_create_table",
    subsystem: Subsystem::Tasks,
    description: "Persist locally discarded cloud-sync rows for recovery (cas-ab2f)",
    up: &[
        "CREATE TABLE IF NOT EXISTS sync_conflicts (id INTEGER PRIMARY KEY AUTOINCREMENT, entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, discarded_row_json TEXT NOT NULL, winner_side TEXT NOT NULL, strategy TEXT NOT NULL, resolved_at TEXT NOT NULL)",
        "CREATE INDEX IF NOT EXISTS idx_sync_conflicts_resolved_at ON sync_conflicts(resolved_at DESC)",
    ],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sync_conflicts')",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_a_recoverable_conflict_journal() {
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
        conn.execute("INSERT INTO sync_conflicts (entity_type, entity_id, discarded_row_json, winner_side, strategy, resolved_at) VALUES ('task', 'cas-a', '{}', 'remote', 'timestamp_lww', '2026-08-09T00:00:00Z')", []).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sync_conflicts", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
