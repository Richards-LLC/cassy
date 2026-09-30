//! Normalize legacy proof-target declarations once without changing task state.

use crate::migration::{Migration, Result, Subsystem};
use rusqlite::{Connection, params};

pub const MIGRATION: Migration = Migration {
    id: 262,
    name: "tasks_normalize_proof_targets",
    subsystem: Subsystem::Tasks,
    description: "Normalize JSON-array and quoted legacy proof-target fragments",
    up: &[],
    // This is a data migration: only its transaction's ledger receipt detects
    // completion. A later cloud import is still normalized by task-store I/O.
    detect: Some(
        "SELECT EXISTS(SELECT 1 FROM cas_migrations WHERE id = 262 AND name = 'tasks_normalize_proof_targets')",
    ),
};

pub(crate) fn normalize_legacy_proof_targets(conn: &Connection) -> Result<()> {
    let rows = {
        let mut stmt =
            conn.prepare("SELECT id, proof_targets FROM tasks WHERE proof_targets IS NOT NULL")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, legacy) in rows {
        let targets = cas_types::parse_proof_targets(Some(&legacy));
        let normalized = cas_types::proof_targets_to_string(&targets);
        if normalized.as_deref() != Some(legacy.as_str()) {
            conn.execute(
                "UPDATE tasks SET proof_targets = ?1 WHERE id = ?2",
                params![normalized, id],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, proof_targets TEXT, status TEXT DEFAULT 'awaiting_merge', updated_at TEXT DEFAULT 'unchanged');
            CREATE TABLE cas_migrations (id INTEGER PRIMARY KEY, name TEXT UNIQUE, subsystem TEXT, applied_at TEXT);").unwrap();
        for (id, value) in [
            (
                "array",
                Some(r#"["cas --lib rules", "cas --lib maintenance_jobs"]"#),
            ),
            (
                "widened",
                Some(r#"["cas --lib rules", "cas --lib maintenance_jobs"], rules, core"#),
            ),
            ("quoted", Some(r#""rules", "core",rules"#)),
            ("csv", Some("rules, core")),
            ("empty", Some(" , ")),
            ("null", None),
            ("canonical", Some(r#"["rules","core"]"#)),
        ] {
            conn.execute(
                "INSERT INTO tasks (id, proof_targets) VALUES (?1, ?2)",
                params![id, value],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn migration_normalizes_legacy_rows_and_records_completion_once() {
        let conn = fixture();
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        crate::migration::apply_migration(&conn, &MIGRATION).unwrap();
        conn.execute_batch("COMMIT").unwrap();
        assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        for (id, expected) in [
            (
                "array",
                Some(r#"["cas --lib rules","cas --lib maintenance_jobs"]"#),
            ),
            (
                "widened",
                Some(r#"["cas --lib rules","cas --lib maintenance_jobs","rules","core"]"#),
            ),
            ("quoted", Some(r#"["rules","core"]"#)),
            ("csv", Some(r#"["rules","core"]"#)),
            ("empty", None),
            ("null", None),
            ("canonical", Some(r#"["rules","core"]"#)),
        ] {
            let actual: Option<String> = conn
                .query_row("SELECT proof_targets FROM tasks WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(actual.as_deref(), expected, "{id}");
        }
        let changed: i64 = conn.query_row("SELECT COUNT(*) FROM tasks WHERE status != 'awaiting_merge' OR updated_at != 'unchanged'", [], |row| row.get(0)).unwrap();
        assert_eq!(changed, 0);
        conn.execute_batch("CREATE TRIGGER no_rewrites BEFORE UPDATE ON tasks BEGIN SELECT RAISE(ABORT,'second rewrite'); END;").unwrap();
        normalize_legacy_proof_targets(&conn).unwrap();
    }

    #[test]
    fn migration_failure_rolls_back_data_and_ledger_together() {
        let conn = fixture();
        conn.execute_batch("CREATE TRIGGER refuse_update BEFORE UPDATE ON tasks WHEN NEW.id = 'csv' BEGIN SELECT RAISE(ABORT,'fixture failure'); END; BEGIN IMMEDIATE;").unwrap();
        assert!(crate::migration::apply_migration(&conn, &MIGRATION).is_err());
        conn.execute_batch("ROLLBACK").unwrap();
        assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
        let array: String = conn
            .query_row(
                "SELECT proof_targets FROM tasks WHERE id='array'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            array,
            r#"["cas --lib rules", "cas --lib maintenance_jobs"]"#
        );
    }
}
