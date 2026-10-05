//! Local operator event/outbox ledger; no enrolled or remote transport yet.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 263,
    name: "operator_delivery_outbox",
    subsystem: Subsystem::Tasks,
    description: "Record immutable local operator turns and bounded delivery claims (cas-4c78)",
    up: cas_store::OPERATOR_DELIVERY_SCHEMA_STATEMENTS,
    detect: Some("SELECT CASE WHEN
        EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_delivery_outbox')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_operator_delivery_pending')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='operator_delivery_immutable')
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn creates_detects_and_reconciles_the_outbox_schema() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        for _ in 0..2 {
            for statement in MIGRATION.up {
                conn.execute_batch(statement).unwrap();
            }
            assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        }
        conn.execute_batch("DROP TRIGGER operator_delivery_immutable")
            .unwrap();
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        for statement in MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        conn.execute("INSERT INTO operator_delivery_outbox(event_id,prompt_id,payload_snapshot,created_at) VALUES ('event',1,'snapshot','now')", []).unwrap();
        assert!(
            conn.execute(
                "UPDATE operator_delivery_outbox SET payload_snapshot = 'different'",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE operator_delivery_outbox SET audience_state = 'enrolled'",
                []
            )
            .is_err()
        );
        conn.execute(
            "UPDATE operator_delivery_outbox SET lease_token = 'token', lease_until_ms = 10",
            [],
        )
        .unwrap();
    }
}
