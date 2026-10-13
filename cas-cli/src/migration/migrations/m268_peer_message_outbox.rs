//! Durable logical peer sends, retried with one cloud idempotency key (cas-13c7).
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 268,
    name: "peer_message_outbox",
    subsystem: Subsystem::Agents,
    description: "Persist peer sends, retry leases, receipts and terminal sender notices",
    up: &[
        "CREATE TABLE IF NOT EXISTS peer_message_outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            sender_agent_id TEXT NOT NULL,
            sender_name TEXT NOT NULL,
            sender_session TEXT,
            recipient TEXT NOT NULL,
            payload TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','retrying','sent','abandoned')),
            created_at INTEGER NOT NULL,
            next_attempt_at INTEGER NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            cloud_id TEXT,
            last_error TEXT,
            notice_sent INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE INDEX IF NOT EXISTS idx_peer_outbox_due ON peer_message_outbox(state,next_attempt_at)",
    ],
    detect: Some("SELECT CASE WHEN EXISTS (SELECT 1 FROM sqlite_master WHERE name='peer_message_outbox' AND type='table') AND EXISTS (SELECT 1 FROM sqlite_master WHERE name='idx_peer_outbox_due' AND type='index') THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cas_13c7_migration_creates_and_reconciles_outbox() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        assert_eq!(
            conn.query_row(MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        for _ in 0..2 {
            for sql in MIGRATION.up {
                conn.execute_batch(sql).unwrap();
            }
        }
        conn.execute("INSERT INTO peer_message_outbox(sender_agent_id,sender_name,recipient,payload,created_at,next_attempt_at) VALUES ('a','sup-a','sup-b','{}',0,0)", []).unwrap();
        assert_eq!(
            conn.query_row("SELECT state FROM peer_message_outbox", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "pending"
        );
        conn.execute_batch("DROP INDEX idx_peer_outbox_due")
            .unwrap();
        assert_eq!(
            conn.query_row(MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        for sql in MIGRATION.up {
            conn.execute_batch(sql).unwrap();
        }
        assert_eq!(
            conn.query_row(MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
