//! Normalize legacy proof-target declarations once without changing task state.

use crate::migration::{Migration, Result, Subsystem};
use rusqlite::{Connection, params};

pub const MIGRATION: Migration = Migration {
    id: 262,
    name: "tasks_normalize_proof_targets",
    subsystem: Subsystem::Tasks,
    description: "Normalize JSON-array and quoted legacy proof-target fragments",
    up: &[],
    // A missing ledger row is also detectable when every declaration already
    // contains unique, trimmed strings in an array (or NULL). Be conservative
    // about quote/bracket prefixes: they may be legacy fragments. The ledger
    // still marks completed repairs of unknown targets preserved by the parser.
    detect: Some(
        "SELECT CASE WHEN EXISTS (
             SELECT 1 FROM cas_migrations WHERE id = 262 AND name = 'tasks_normalize_proof_targets'
         ) OR NOT EXISTS (
             SELECT 1 FROM tasks WHERE proof_targets IS NOT NULL AND CASE
                 WHEN NOT json_valid(proof_targets) THEN 1
                 WHEN json_type(proof_targets) != 'array' THEN 1
                 ELSE json_array_length(proof_targets) = 0
                     OR EXISTS (
                         SELECT 1 FROM json_each(proof_targets) WHERE type != 'text'
                             OR value = ''
                             OR value != trim(value, char(9,10,11,12,13,32,133,160,5760,
                                 8192,8193,8194,8195,8196,8197,8198,8199,8200,8201,8202,
                                 8232,8233,8239,8287,12288))
                             OR substr(value,1,1) IN ('[',char(34))
                     )
                     OR (SELECT COUNT(DISTINCT value) FROM json_each(proof_targets))
                         != json_array_length(proof_targets)
             END
         ) THEN 1 ELSE 0 END",
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
        conn.execute("DELETE FROM cas_migrations WHERE id=262", [])
            .unwrap();
        assert!(
            crate::migration::migration_is_detected(&conn, &MIGRATION),
            "a stripped ledger must still detect normalized task data"
        );
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
    fn detection_accepts_clean_data_without_a_ledger_and_refuses_legacy_values() {
        let conn = fixture();
        conn.execute("DELETE FROM tasks", []).unwrap();
        assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
        conn.execute("INSERT INTO tasks (id) VALUES ('probe')", [])
            .unwrap();
        for (value, detected) in [
            (None, true),
            (Some(r#"["rules","core"]"#), true),
            (Some(r#"["target,with,commas","quoted\"target"]"#), true),
            (Some(r#"["rules", "core"]"#), true),
            (Some("rules,core"), false),
            (Some(r#""rules", "core""#), false),
            (Some(r#"["rules","core"],updates"#), false),
            (Some(r#"["rules","rules"]"#), false),
            (Some(r#"[" rules "]"#), false),
            (Some(r#"["\u2003rules"]"#), false),
            (Some(r#"["[\"rules\"", "\"core\"]"]"#), false),
            (Some(r#"["\"rules\""]"#), false),
            (Some(r#"["", "core"]"#), false),
            (Some("[]"), false),
            (Some("[null]"), false),
            (Some("[1]"), false),
            (Some("{}"), false),
            (Some("null"), false),
        ] {
            conn.execute(
                "UPDATE tasks SET proof_targets=?1 WHERE id='probe'",
                [value],
            )
            .unwrap();
            assert_eq!(
                crate::migration::migration_is_detected(&conn, &MIGRATION),
                detected,
                "{value:?}"
            );
        }
        // A completed normalization retains unknown malformed target names.
        conn.execute(
            "UPDATE tasks SET proof_targets='[null]' WHERE id='probe'",
            [],
        )
        .unwrap();
        crate::migration::apply_migration(&conn, &MIGRATION).unwrap();
        assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
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
