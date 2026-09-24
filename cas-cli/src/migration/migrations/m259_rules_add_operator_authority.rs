//! Migration: add `operator_authority` to the `rules` table (cas-5372, GH #990).
//!
//! An operator hard rule takes effect the day it is recorded only when the
//! operator or a registered supervisor authorised it in this project. The
//! column holds `<author>|<sha256 of the authorised content>`; it is local-only
//! and never synced. Fresh DBs get it via CREATE TABLE; the detect query makes
//! this idempotent for older DBs.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 259,
    name: "rules_add_operator_authority",
    subsystem: Subsystem::Rules,
    description: "Add operator_authority TEXT column to rules for operator hard-rule authorisation",
    up: &["ALTER TABLE rules ADD COLUMN operator_authority TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('rules') WHERE name = 'operator_authority') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn detected(conn: &Connection) -> i64 {
        conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn adds_and_detects_the_operator_authority_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE rules (id TEXT PRIMARY KEY, created TEXT NOT NULL, content TEXT NOT NULL DEFAULT '');
             INSERT INTO rules (id, created, content) VALUES ('rule-1', '2026-09-24', 'HARD RULE: x');",
        )
        .unwrap();
        assert_eq!(detected(&conn), 0);
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        assert_eq!(detected(&conn), 1);
        let authority: Option<String> = conn
            .query_row("SELECT operator_authority FROM rules WHERE id = 'rule-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(authority, None, "existing rules start unauthorised");
    }
}
