//! Spec §11 — the eight hard asserts M3 must pass before extraction.
//!
//! Every one of these aborts. None warns. The point of the list is that each
//! item is a surface with *no destination*: if it is populated at run time,
//! migrating around it would be a silent drop of data the inventory proved does
//! not exist today.
//!
//! Asserts 6 and 7 are enforced structurally elsewhere and are not re-checked
//! here: 6 (no `Store::list()`) by [`super::source::extract_all`]'s keyset
//! pagination, 7 (no unrouted row) by [`super::routing::route`] returning an
//! error. Assert 8 (every carry-verbatim page ends locked) can only be checked
//! after the write and lives in [`super::apply`].

use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::Connection;

/// Tables that must be empty: a populated one means an edge type with no
/// destination (§5.2 items 2.16, 2.17).
const MUST_BE_EMPTY: [&str; 4] = [
    "code_memory_links",
    "entities",
    "entity_mentions",
    "relationships",
];

/// Does a table exist? An absent table is legitimately empty — an older
/// database simply never had it — but a *failed* query is not.
fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn count_of(conn: &Connection, table: &str) -> Result<i64> {
    if !table_exists(conn, table)? {
        return Ok(0);
    }
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?,
    )
}

/// §11 assert 1 — stranded `sync_queue` entry rows (§5.3).
///
/// Returned rather than asserted here: the dry run's job is to *report* a dirty
/// queue, and only an apply may not proceed past one.
pub fn sync_queue_entry_count(conn: &Connection) -> Result<i64> {
    if !table_exists(conn, "sync_queue")? {
        return Ok(0);
    }
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sync_queue WHERE entity_type = 'entry'",
        [],
        |row| row.get(0),
    )?)
}

/// §11 asserts 2, 3, 4 — the structural preconditions. These abort in dry-run
/// mode too: if they fail, the audit the dry run would print is not
/// trustworthy.
pub fn check_structural(conn: &Connection, cas_root: &Path, label: &str) -> Result<()> {
    // Assert 2 — a memory↔code-symbol edge or graph node has no destination.
    for table in MUST_BE_EMPTY {
        let count = count_of(conn, table)?;
        if count > 0 {
            bail!(
                "[{label}] {table} holds {count} row(s), which have no destination in the \
                 knowledge system — migration aborts rather than drop them (spec §5.2)"
            );
        }
    }

    // Assert 3 — a compressed row would lose `raw_content` on the way through.
    let compressed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entries WHERE compressed = 1 OR raw_content IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    if compressed > 0 {
        bail!(
            "[{label}] {compressed} entr(ies) are compressed or carry raw_content; the \
             migration has no carrier for it and aborts rather than drop it (spec §5.2 item 2.8)"
        );
    }

    // Assert 4 — this migration is defined for the SQLite backend only.
    for dir in ["entries", "archive"] {
        let path = cas_root.join(dir);
        if path.is_dir() {
            bail!(
                "[{label}] legacy MarkdownStore directory {} exists; this migration is \
                 scoped to the SQLite backend and aborts rather than pretend the other \
                 backend does not exist (spec §5.2 item 2.18)",
                path.display()
            );
        }
    }

    Ok(())
}

/// §11 assert 5 / Rule D0 — the source databases are live (PROJECT moved
/// 1245 → 1246 rows mid-session while the inventory was being written), so the
/// row count must be stable across two reads or the audit is measuring a
/// moving target.
pub fn assert_stable_count(before: i64, after: i64, label: &str) -> Result<()> {
    if before != after {
        bail!(
            "[{label}] source row count changed during extraction ({before} → {after}); \
             the database is live. Re-run against a quiesced copy — a loss audit over a \
             moving corpus proves nothing (spec §3 Rule D0)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id TEXT PRIMARY KEY, compressed INTEGER NOT NULL DEFAULT 0, raw_content TEXT);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn absent_tables_count_as_empty() {
        let conn = scratch();
        assert_eq!(count_of(&conn, "entities").unwrap(), 0);
        assert_eq!(sync_queue_entry_count(&conn).unwrap(), 0);
    }

    #[test]
    fn a_populated_graph_table_aborts() {
        let conn = scratch();
        conn.execute_batch("CREATE TABLE entities (id TEXT); INSERT INTO entities VALUES ('x');")
            .unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let err = check_structural(&conn, temp.path(), "project")
            .unwrap_err()
            .to_string();
        assert!(err.contains("entities"), "{err}");
    }

    #[test]
    fn raw_content_without_the_compressed_flag_still_aborts() {
        let conn = scratch();
        conn.execute(
            "INSERT INTO entries (id, raw_content) VALUES ('a', 'x')",
            [],
        )
        .unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let err = check_structural(&conn, temp.path(), "project")
            .unwrap_err()
            .to_string();
        assert!(err.contains("raw_content"), "{err}");
    }

    #[test]
    fn a_markdown_store_directory_aborts() {
        let conn = scratch();
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("archive")).unwrap();
        let err = check_structural(&conn, temp.path(), "global")
            .unwrap_err()
            .to_string();
        assert!(err.contains("MarkdownStore"), "{err}");
    }

    #[test]
    fn a_clean_database_passes() {
        let conn = scratch();
        let temp = tempfile::TempDir::new().unwrap();
        check_structural(&conn, temp.path(), "project").unwrap();
    }

    #[test]
    fn a_moving_row_count_aborts() {
        assert!(assert_stable_count(1245, 1245, "project").is_ok());
        let err = assert_stable_count(1245, 1246, "project")
            .unwrap_err()
            .to_string();
        assert!(err.contains("1245 → 1246"), "{err}");
    }
}
