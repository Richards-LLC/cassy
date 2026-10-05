//! Application storage receipts, independent of m263's cloud outbox.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 264,
    name: "operator_reply_device_receipts",
    subsystem: Subsystem::Tasks,
    description: "Record per-device Commander reply persistence without marking operator read",
    up: &[cas_store::OPERATOR_REPLY_RECEIPTS_SCHEMA],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_reply_device_receipts') AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_operator_reply_receipt_device')",
    ),
};

#[cfg(test)]
mod tests {
    #[test]
    fn receipt_migration_is_idempotent_without_cloud_outbox_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE prompt_queue(id INTEGER PRIMARY KEY);")
            .unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute_batch(sql).unwrap();
            }
            assert_eq!(
                conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }
}
