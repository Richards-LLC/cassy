//! Migration: distinguish unresolved retrieval telemetry from negative evidence.
//!
//! Rows recorded as `ignored` before this migration were produced by the Stop
//! default and therefore mean "unresolved" historically. They are preserved
//! verbatim: rewriting them would invent certainty the old instrumentation did
//! not capture. New automatic Stop rows use `unresolved`; `ignored` remains
//! available only for explicit evidence of non-use.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 244,
    name: "retrieval_outcomes_add_unresolved",
    subsystem: Subsystem::Events,
    description: "Allow unresolved retrieval outcomes without rewriting historical ignored rows (cas-dd4e)",
    up: &[
        "CREATE TABLE retrieval_outcomes_new (
            id TEXT PRIMARY KEY,
            query_id TEXT NOT NULL,
            result_id TEXT NOT NULL,
            outcome TEXT NOT NULL CHECK (
                outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful', 'unresolved')
            ),
            actor_hash TEXT NOT NULL,
            session_hash TEXT NOT NULL,
            correction_ref TEXT,
            created_at TEXT NOT NULL,
            FOREIGN KEY (query_id, result_id)
                REFERENCES retrieval_query_results(query_id, result_id) ON DELETE CASCADE
        )",
        "INSERT OR IGNORE INTO retrieval_outcomes_new
            (id, query_id, result_id, outcome, actor_hash, session_hash, correction_ref, created_at)
            SELECT id, query_id, result_id, outcome, actor_hash, session_hash, correction_ref, created_at
            FROM retrieval_outcomes",
        "DROP TABLE retrieval_outcomes",
        "ALTER TABLE retrieval_outcomes_new RENAME TO retrieval_outcomes",
        "CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_query
            ON retrieval_outcomes(query_id, result_id)",
        "CREATE INDEX IF NOT EXISTS idx_retrieval_outcomes_created
            ON retrieval_outcomes(created_at)",
    ],
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'retrieval_outcomes'
           AND sql LIKE '%''unresolved''%'",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    #[test]
    fn migration_preserves_legacy_rows_and_accepts_unresolved() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE retrieval_queries (
                id TEXT PRIMARY KEY,
                query_fingerprint TEXT NOT NULL,
                query_family TEXT NOT NULL,
                ranking_policy TEXT NOT NULL,
                session_hash TEXT,
                created_at TEXT NOT NULL
             );
             CREATE TABLE retrieval_query_results (
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                document_type TEXT NOT NULL,
                rank INTEGER NOT NULL,
                PRIMARY KEY (query_id, result_id),
                FOREIGN KEY (query_id) REFERENCES retrieval_queries(id) ON DELETE CASCADE
             );
             CREATE TABLE retrieval_outcomes (
                id TEXT PRIMARY KEY,
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                outcome TEXT NOT NULL CHECK (
                    outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful')
                ),
                actor_hash TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                correction_ref TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (query_id, result_id)
                    REFERENCES retrieval_query_results(query_id, result_id) ON DELETE CASCADE
             );
             INSERT INTO retrieval_queries VALUES
                ('q1', 'fingerprint', 'ambient_transition', 'policy', 'session', 'now');
             INSERT INTO retrieval_query_results VALUES ('q1', 'entry-1', 'entry', 0);
             INSERT INTO retrieval_outcomes VALUES
                ('old-default', 'q1', 'entry-1', 'ignored', 'actor', 'session', NULL, 'now');",
        )
        .unwrap();

        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }

        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT outcome FROM retrieval_outcomes WHERE id = 'old-default'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "ignored"
        );
        conn.execute(
            "INSERT OR IGNORE INTO retrieval_outcomes VALUES
             (?1, 'q1', 'entry-1', 'unresolved', 'actor', 'session', NULL, 'now')",
            params!["new-default"],
        )
        .unwrap();
    }

    #[test]
    fn migration_preserves_existing_judge_attribution() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE retrieval_queries (
                id TEXT PRIMARY KEY,
                query_fingerprint TEXT NOT NULL,
                query_family TEXT NOT NULL,
                ranking_policy TEXT NOT NULL,
                session_hash TEXT,
                created_at TEXT NOT NULL
             );
             CREATE TABLE retrieval_query_results (
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                document_type TEXT NOT NULL,
                rank INTEGER NOT NULL,
                PRIMARY KEY (query_id, result_id)
             );
             CREATE TABLE retrieval_outcomes (
                id TEXT PRIMARY KEY,
                query_id TEXT NOT NULL,
                result_id TEXT NOT NULL,
                outcome TEXT NOT NULL CHECK (
                    outcome IN ('used', 'helpful', 'ignored', 'corrected', 'harmful')
                ),
                actor_hash TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                correction_ref TEXT,
                created_at TEXT NOT NULL,
                attribution TEXT NOT NULL DEFAULT 'explicit'
             );
             INSERT INTO retrieval_queries VALUES
                ('q1', 'fingerprint', 'ambient_transition', 'policy', 'session', 'now');
             INSERT INTO retrieval_query_results VALUES ('q1', 'entry-1', 'entry', 0);
             INSERT INTO retrieval_outcomes
                (id, query_id, result_id, outcome, actor_hash, session_hash,
                 correction_ref, created_at, attribution)
             VALUES
                ('judge-row', 'q1', 'entry-1', 'helpful', 'actor', 'session',
                 NULL, 'now', 'judge');",
        )
        .unwrap();

        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }

        let attribution: String = conn
            .query_row(
                "SELECT attribution FROM retrieval_outcomes WHERE id = 'judge-row'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attribution, "judge");
    }
}
