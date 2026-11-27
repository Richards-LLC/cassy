use crate::migration::{Migration, Result, Subsystem};
use rusqlite::Connection;

pub const MIGRATION: Migration = Migration {
    id: 303,
    name: "entries_purge_session_noise",
    subsystem: Subsystem::Entries,
    description: "Back up and delete automatic session summaries and prompt memories with their queued copies",
    up: &[
        "CREATE TABLE IF NOT EXISTS entries_purge_backup_m303 AS SELECT * FROM entries WHERE 0",
        "INSERT INTO entries_purge_backup_m303 SELECT * FROM entries
         WHERE EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(entries.tags) THEN entries.tags
                             ELSE '[' || replace(json_quote(entries.tags), ',', '\",\"') || ']' END)
                       WHERE trim(value) IN ('synthesized', 'user-prompt'))
           AND id NOT IN (SELECT id FROM entries_purge_backup_m303)",
        "DELETE FROM entries WHERE id IN (SELECT id FROM entries_purge_backup_m303)",
    ],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'entries_purge_backup_m303')",
    ),
};

pub(crate) fn purge_queue(conn: &Connection) -> Result<()> {
    if crate::migration::detector::table_exists(conn, "sync_queue") {
        conn.execute(
            "DELETE FROM sync_queue WHERE entity_type = 'entry' AND (
                 entity_id IN (SELECT id FROM entries_purge_backup_m303)
                 OR EXISTS (
                     SELECT 1 FROM json_each(
                         CASE WHEN json_valid(payload) THEN payload ELSE '{}' END, '$.tags'
                     ) WHERE value IN ('synthesized', 'user-prompt')
                 )
             )",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cloud::{EntityType, SyncOperation, SyncQueue};
    use crate::migration::run_migrations;
    use crate::store::{SqliteStore, Store};
    use crate::types::Entry;
    use rusqlite::{Connection, params};
    use tempfile::TempDir;

    #[test]
    fn upgrade_purges_noise_and_queued_copies_preserving_deliberate_memories() {
        for with_queue in [false, true] {
            let temp = TempDir::new().unwrap();
            let conn = Connection::open(temp.path().join("cas.db")).unwrap();
            conn.execute_batch(cas_store::ENTRIES_RULES_SCHEMA).unwrap();
            conn.execute_batch(cas_store::TASK_SCHEMA).unwrap();
            assert!(
                run_migrations(temp.path(), false)
                    .unwrap()
                    .errors
                    .is_empty()
            );
            let conn = Connection::open(temp.path().join("cas.db")).unwrap();
            conn.execute("DELETE FROM cas_migrations WHERE id = 303", [])
                .unwrap();
            conn.execute("DROP TABLE entries_purge_backup_m303", [])
                .unwrap();
            let store = SqliteStore::open(temp.path()).unwrap();
            store.init().unwrap();
            for (id, tag) in [
                ("error", "synthesized"),
                ("archived", "user-prompt"),
                ("deliberate", "chosen"),
                ("similar-tag", "not-synthesized"),
            ] {
                let mut entry = Entry::new(id.to_string(), id.to_string());
                entry.tags = vec![tag.to_string()];
                store.add(&entry).unwrap();
            }
            store.archive("archived").unwrap();
            conn.execute(
                "UPDATE entries SET tags = ' session-errors, synthesized ' WHERE id = 'error'",
                [],
            )
            .unwrap();
            conn.execute("UPDATE entries SET tags = '' WHERE id = 'deliberate'", [])
                .unwrap();
            if with_queue {
                let queue = SyncQueue::open(temp.path()).unwrap();
                queue.init().unwrap();
                for team in ["", "team-1"] {
                    for (kind, id, operation, payload) in [
                        (
                            EntityType::Entry,
                            "error",
                            SyncOperation::Upsert,
                            Some("{}"),
                        ),
                        (EntityType::Entry, "archived", SyncOperation::Delete, None),
                        (
                            EntityType::Entry,
                            "orphan",
                            SyncOperation::Upsert,
                            Some(r#"{"tags":["synthesized"]}"#),
                        ),
                        (
                            EntityType::Entry,
                            "orphan-prompt",
                            SyncOperation::Upsert,
                            Some(r#"{"tags":["user-prompt"]}"#),
                        ),
                        (
                            EntityType::Entry,
                            "deliberate",
                            SyncOperation::Upsert,
                            Some("{}"),
                        ),
                        (
                            EntityType::Entry,
                            "invalid",
                            SyncOperation::Upsert,
                            Some("invalid-json"),
                        ),
                        (EntityType::Task, "error", SyncOperation::Upsert, Some("{}")),
                    ] {
                        queue
                            .enqueue_for_team(kind, id, operation, payload, team)
                            .unwrap();
                    }
                }
            }
            let result = run_migrations(temp.path(), false).unwrap();
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert!(
                result
                    .applied_names
                    .iter()
                    .any(|name| name == super::MIGRATION.name)
            );
            let ids: Vec<String> = conn
                .prepare("SELECT id FROM entries ORDER BY id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            assert_eq!(ids, ["deliberate", "similar-tag"]);
            if with_queue {
                let remaining: i64 = conn
                    .query_row("SELECT COUNT(*) FROM sync_queue", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(remaining, 6);
                for id in ["error", "archived", "orphan", "orphan-prompt"] {
                    let queued: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM sync_queue WHERE entity_type = 'entry' AND entity_id = ?1",
                        params![id], |row| row.get(0),
                    ).unwrap();
                    assert_eq!(queued, 0);
                }
            }
            let again = run_migrations(temp.path(), false).unwrap();
            assert!(again.errors.is_empty(), "{:?}", again.errors);
            assert_eq!(again.applied_count, 0);
        }
    }
}
