//! Preserve verbatim stakeholder provenance; old rules remain unattributed.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 294,
    name: "rules_stakeholder",
    subsystem: Subsystem::Rules,
    description: "Add nullable stakeholder attribution to rules without backfilling old rows",
    up: &["ALTER TABLE rules ADD COLUMN stakeholder TEXT"],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('rules') WHERE name = 'stakeholder')",
    ),
};

#[cfg(test)]
mod tests {
    #[test]
    fn attribution_migration_preserves_existing_rules_without_backfill() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE rules (id TEXT PRIMARY KEY, content TEXT NOT NULL); INSERT INTO rules VALUES ('legacy', 'Existing instruction');").unwrap();
        let detected = || {
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(detected(), 0);
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(detected(), 1);
        let (content, attribution): (String, Option<String>) = conn
            .query_row(
                "SELECT content, stakeholder FROM rules WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // pin: An additive provenance migration must preserve the prior instruction verbatim.
        assert_eq!(content, "Existing instruction");
        assert!(attribution.is_none());
    }
}
