//! Per-row server revisions, the authority for sync conflict resolution.
//!
//! The cloud maintains a monotonic `revision` per row and increments it on
//! every accepted write. Comparing revisions answers "which side is newer"
//! without trusting any machine's clock, so a laptop an hour behind can no
//! longer silently win or lose a conflict.
//!
//! This is a ledger rather than a column on each entity table: revisions apply
//! to fourteen entity types across five stores, and the server strips the field
//! from the stored body precisely because it is transport metadata, not part of
//! the entity. Keeping it beside the sync queue also keeps it next to the push
//! machinery that has to send it back as the base revision.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};

use crate::cloud::sync_queue::{EntityType, SyncQueue};
use crate::error::CasError;

/// DDL for the revision ledger. Shared by the queue schema (fresh databases)
/// and migration 250 (existing ones).
pub const SYNC_REVISION_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sync_revisions (
        entity_type TEXT NOT NULL,
        entity_id TEXT NOT NULL,
        revision INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (entity_type, entity_id)
    )",
];

/// Read a revision off the wire.
///
/// The cloud sends `revision` as a DECIMAL STRING (`"10"`), and also accepts a
/// JSON number. Parsing is mandatory rather than cosmetic: a lexicographic
/// comparison ranks `"9"` above `"10"`, which would invert the winner of every
/// conflict from a row's tenth edit onward.
///
/// Anything else — a float, a negative value, a non-numeric string, `null` —
/// yields `None`, which callers treat as "this row has no revision" and fall
/// back to the timestamp path. Guessing a number here would be worse than
/// having none: the server drops a row whose revision it cannot parse.
pub fn parse_wire_revision(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::String(raw) => {
            let trimmed = raw.trim();
            (!trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| trimmed.parse::<i64>().ok())
                .flatten()
        }
        serde_json::Value::Number(number) => number.as_i64().filter(|value| *value >= 0),
        _ => None,
    }
}

/// Pull the `revision` field out of a raw pulled row.
pub fn wire_revision(raw: &serde_json::Value) -> Option<i64> {
    parse_wire_revision(raw.get("revision"))
}

impl SyncQueue {
    /// Record the server revision observed for one row.
    ///
    /// Monotonic by construction: a replayed older envelope must not roll the
    /// ledger backwards, because the base revision we later send is what
    /// decides whether our push is accepted at all.
    pub fn record_revision(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        revision: i64,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO sync_revisions (entity_type, entity_id, revision, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(entity_type, entity_id) DO UPDATE SET
                revision = MAX(sync_revisions.revision, excluded.revision),
                updated_at = excluded.updated_at
            "#,
            params![
                entity_type.as_str(),
                entity_id,
                revision,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// The last server revision this client observed for a row, if any.
    pub fn revision(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<Option<i64>, CasError> {
        let conn = self.conn.lock().unwrap();
        let revision: Option<i64> = conn
            .query_row(
                "SELECT revision FROM sync_revisions WHERE entity_type = ?1 AND entity_id = ?2",
                params![entity_type.as_str(), entity_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(revision)
    }

    /// Forget a row's revision, so the next push falls back to the timestamp
    /// path instead of sending a base the server will reject forever.
    pub fn clear_revision(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sync_revisions WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type.as_str(), entity_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_revisions_parse_as_numbers_not_strings() {
        // The regression this guards: "9" sorts above "10" lexicographically,
        // so a string compare would hand every conflict to the older row once
        // a row has been edited ten times.
        assert_eq!(parse_wire_revision(Some(&serde_json::json!("10"))), Some(10));
        assert_eq!(parse_wire_revision(Some(&serde_json::json!("9"))), Some(9));
        assert!(
            parse_wire_revision(Some(&serde_json::json!("10")))
                > parse_wire_revision(Some(&serde_json::json!("9")))
        );
        assert_eq!(parse_wire_revision(Some(&serde_json::json!(7))), Some(7));
        assert_eq!(parse_wire_revision(Some(&serde_json::json!("0"))), Some(0));
    }

    #[test]
    fn unparseable_revisions_are_absent_rather_than_guessed() {
        for value in [
            serde_json::json!(null),
            serde_json::json!(""),
            serde_json::json!("  "),
            serde_json::json!("-1"),
            serde_json::json!(-1),
            serde_json::json!("1.5"),
            serde_json::json!(1.5),
            serde_json::json!("abc"),
            serde_json::json!("12a"),
            serde_json::json!(true),
            serde_json::json!({"revision": "1"}),
        ] {
            assert_eq!(
                parse_wire_revision(Some(&value)),
                None,
                "must not invent a revision for {value}"
            );
        }
        assert_eq!(parse_wire_revision(None), None);
        assert_eq!(wire_revision(&serde_json::json!({"id": "cas-a"})), None);
        assert_eq!(
            wire_revision(&serde_json::json!({"id": "cas-a", "revision": "4"})),
            Some(4)
        );
    }
}
