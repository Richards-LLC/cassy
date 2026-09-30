//! Persist recorded-only task reversibility; no policy reads this field.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 261,
    name: "tasks_add_door",
    subsystem: Subsystem::Tasks,
    description: "Add optional recorded-only task door declaration",
    up: &[
        "ALTER TABLE tasks ADD COLUMN door TEXT CHECK (door IS NULL OR door IN ('one-way', 'two-way'))",
    ],
    detect: Some("SELECT EXISTS (SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'door')"),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    #[test]
    fn legacy_rows_keep_nullable_door_and_invalid_values_are_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (id TEXT PRIMARY KEY); INSERT INTO tasks VALUES ('legacy');",
        )
        .unwrap();
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        assert_eq!(
            conn.query_row("SELECT door FROM tasks", [], |row| row
                .get::<_, Option<String>>(0))
                .unwrap(),
            None
        );
        for door in ["one-way", "two-way"] {
            conn.execute("UPDATE tasks SET door=?1", [door]).unwrap();
        }
        assert!(conn.execute("UPDATE tasks SET door='unknown'", []).is_err());
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
