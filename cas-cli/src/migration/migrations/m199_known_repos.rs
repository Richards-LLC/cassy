//! Migration: Create the `known_repos` table on the host `~/.cas/cas.db`.
//!
//! Registers every Cassy-aware repo directory the host has touched so a
//! cross-repo sweep (Unit 4 `cas sweep-all`, Unit 3's opportunistic sweep)
//! can list every candidate without a filesystem scan. Writers: `cas init`,
//! factory daemon startup, MCP server startup when `.cas/` exists in CWD.
//!
//! See `docs/brainstorms/2026-04-21-worktree-leak-and-supervisor-discipline-spike-a.md`.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 199,
    name: "known_repos_create_table",
    subsystem: Subsystem::Worktrees,
    description: "Create known_repos table for host-scoped repo registry (Unit 4, EPIC cas-7c88)",
    up: &[
        "CREATE TABLE IF NOT EXISTS known_repos (\
            path TEXT PRIMARY KEY, \
            first_seen_at TEXT NOT NULL, \
            last_touched_at TEXT NOT NULL, \
            touch_count INTEGER NOT NULL DEFAULT 1\
        )",
        "CREATE INDEX IF NOT EXISTS idx_known_repos_last_touched \
         ON known_repos(last_touched_at DESC)",
    ],
    detect: Some(
        "SELECT CASE WHEN EXISTS (\
            SELECT 1 FROM sqlite_master \
            WHERE type = 'table' AND name = 'known_repos'\
        ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn apply_up(conn: &Connection) {
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        count > 0
    }

    fn index_exists(conn: &Connection, name: &str) -> bool {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        count > 0
    }

    #[test]
    fn fresh_db_gains_table_and_index() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!table_exists(&conn, "known_repos"));

        apply_up(&conn);

        assert!(table_exists(&conn, "known_repos"));
        assert!(index_exists(&conn, "idx_known_repos_last_touched"));
    }

    #[test]
    fn detect_returns_0_then_1() {
        let conn = Connection::open_in_memory().unwrap();
        let before: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0);

        apply_up(&conn);

        let after: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, 1);
    }

    #[test]
    fn idempotent_when_already_present() {
        let conn = Connection::open_in_memory().unwrap();
        apply_up(&conn);
        apply_up(&conn); // IF NOT EXISTS — must not throw

        // Row survives a second "up"
        conn.execute(
            "INSERT INTO known_repos (path, first_seen_at, last_touched_at, touch_count) \
             VALUES ('/tmp/repo', '2026-04-21T00:00:00Z', '2026-04-21T00:00:00Z', 1)",
            [],
        )
        .unwrap();
        apply_up(&conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM known_repos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
