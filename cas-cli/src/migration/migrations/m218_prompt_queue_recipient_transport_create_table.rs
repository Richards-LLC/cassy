//! Migration: Register worker inbox per-recipient transport state.
//!
//! cas-ac7e (GH #130) installs this table as a prompt-store `init()` side
//! effect, exactly as cas-2816 first did for `prompt_queue_recipient_seen`.
//! Recording the same table and index in the migration ledger makes upgrades
//! and bootstrap detection explicit — without it, a store that predates the
//! change and one that postdates it both report the same migration id while
//! only one of them actually has the table. The `IF NOT EXISTS` statements
//! preserve databases that already received the init-side-effect schema.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 218,
    name: "prompt_queue_recipient_transport_create_table",
    subsystem: Subsystem::Agents,
    description: "Create per-recipient transport state corroborating stage=delivered (cas-ac7e)",
    up: &[
        "CREATE TABLE IF NOT EXISTS prompt_queue_recipient_transport (
            prompt_id INTEGER NOT NULL,
            recipient TEXT NOT NULL,
            delivered_at TEXT NOT NULL,
            PRIMARY KEY (prompt_id, recipient)
        )",
        "CREATE INDEX IF NOT EXISTS idx_prompt_queue_recipient_transport_recipient
            ON prompt_queue_recipient_transport(recipient, prompt_id)",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'prompt_queue_recipient_transport'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'index'
                  AND name = 'idx_prompt_queue_recipient_transport_recipient'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn shape(conn: &Connection) -> (Vec<String>, Vec<String>) {
        let columns = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM pragma_table_info('prompt_queue_recipient_transport')
                     ORDER BY cid",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let indexes = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master
                     WHERE type = 'index'
                       AND tbl_name = 'prompt_queue_recipient_transport'
                       AND sql IS NOT NULL
                     ORDER BY name",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        (columns, indexes)
    }

    #[test]
    fn migration_creates_and_detects_recipient_transport_schema() {
        let conn = Connection::open_in_memory().unwrap();
        let before: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0);

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let after: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, 1);
    }

    /// The ledger and the baseline schema must describe the same table. If they
    /// drift, a migrated store and a freshly-initialised one disagree about the
    /// shape of the evidence `stage=delivered` now depends on.
    #[test]
    fn baseline_and_migration_recipient_transport_shapes_match() {
        let temp = TempDir::new().unwrap();
        let baseline_store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        baseline_store.init().unwrap();
        let baseline = Connection::open(temp.path().join("cas.db")).unwrap();

        let migrated = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        assert_eq!(shape(&baseline), shape(&migrated));
    }
}
