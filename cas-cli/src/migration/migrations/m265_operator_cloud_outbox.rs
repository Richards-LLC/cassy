//! Enrolled cloud delivery lane for operator turns (cas-9b7d). Builds on
//! m263's immutable local outbox; never relabels or backfills its rows.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 265,
    name: "operator_cloud_outbox",
    subsystem: Subsystem::Tasks,
    description: "Record the verified operator feed binding and sealed cloud delivery state (cas-9b7d)",
    up: cas_store::OPERATOR_CLOUD_SCHEMA_STATEMENTS,
    detect: Some("SELECT CASE WHEN
        EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_feed_binding')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_cloud_outbox')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_operator_cloud_pending')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='operator_cloud_identity_immutable')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='operator_cloud_receipt_final')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='operator_cloud_follows_local_purge')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_command_admissions')
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_operator_command_receipt_pending')
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn with_m263(conn: &Connection) {
        for statement in cas_store::OPERATOR_DELIVERY_SCHEMA_STATEMENTS {
            conn.execute_batch(statement).unwrap();
        }
    }

    #[test]
    fn creates_detects_and_reconciles_the_cloud_outbox_schema() {
        let conn = Connection::open_in_memory().unwrap();
        with_m263(&conn);
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        for _ in 0..2 {
            for statement in MIGRATION.up {
                conn.execute_batch(statement).unwrap();
            }
            assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        }
        conn.execute_batch("DROP TRIGGER operator_cloud_receipt_final")
            .unwrap();
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        for statement in MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
    }

    #[test]
    fn audience_is_immutable_receipts_are_final_and_local_purge_cascades() {
        let conn = Connection::open_in_memory().unwrap();
        with_m263(&conn);
        for statement in MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        conn.execute("INSERT INTO operator_delivery_outbox(event_id,prompt_id,payload_snapshot,created_at) VALUES ('event',1,'snapshot','now')", []).unwrap();
        conn.execute("INSERT INTO operator_cloud_outbox(event_id,account_id,machine_id,hub_id,project_id,session_id,created_at) VALUES ('event','acct','mch','hub','proj','s_x','now')", []).unwrap();
        assert!(
            conn.execute("UPDATE operator_cloud_outbox SET account_id = 'other'", [])
                .is_err()
        );
        // A receipt needs sealed bytes.
        assert!(
            conn.execute(
                "UPDATE operator_cloud_outbox SET receipt_kind = 'stored', receipt_at = 'now'",
                []
            )
            .is_err()
        );
        conn.execute("UPDATE operator_cloud_outbox SET feed_generation='1', key_epoch='1', ciphertext='c', digest='d', receipt_kind='stored', receipt_at='now'", []).unwrap();
        assert!(
            conn.execute("UPDATE operator_cloud_outbox SET ciphertext = 'x'", [])
                .is_err()
        );
        conn.execute("DELETE FROM operator_delivery_outbox", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM operator_cloud_outbox", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(left, 0);
    }
}
