//! Migration: add explicit host-local bindings for ambiguous repo selectors.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 214,
    name: "known_repo_bindings",
    subsystem: Subsystem::Worktrees,
    description: "Add explicit host-local WorkTarget selector bindings (cas-4afd)",
    up: &[
        "CREATE TABLE IF NOT EXISTS known_repo_bindings (
            selector TEXT PRIMARY KEY COLLATE BINARY,
            repo_root TEXT NOT NULL,
            git_common_dir TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_known_repo_bindings_updated
            ON known_repo_bindings(updated_at DESC)",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'known_repo_bindings')
            AND EXISTS (SELECT 1 FROM sqlite_master
                        WHERE type = 'index' AND name = 'idx_known_repo_bindings_updated')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_is_idempotent_and_preserves_exact_selector_keys() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
        }
        conn.execute(
            "INSERT INTO known_repo_bindings
             (selector, repo_root, git_common_dir, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            (
                "project:Case-Sensitive",
                "/host/repo",
                "/host/repo/.git",
                "2026-07-30T00:00:00Z",
            ),
        )
        .unwrap();
        let selector: String = conn
            .query_row("SELECT selector FROM known_repo_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(selector, "project:Case-Sensitive");
    }
}
