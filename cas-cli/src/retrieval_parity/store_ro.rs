//! Strictly read-only access to the legacy memory store.
//!
//! The retrieval-parity harness must never mutate the corpus it is measuring:
//! a baseline captured by a run that wrote to the store is not a baseline of
//! the pre-migration system, it is a baseline of whatever the harness left
//! behind. Read-only-ness here is enforced *by construction* rather than by
//! discipline — the connection is opened with `SQLITE_OPEN_READ_ONLY` and
//! without `SQLITE_OPEN_CREATE`, so:
//!
//! * any `INSERT`/`UPDATE`/`DELETE`/`CREATE` reaching this connection fails
//!   with `SQLITE_READONLY` instead of silently succeeding, and
//! * a missing database is an error rather than a freshly-created empty file
//!   that would cheerfully report "zero regressions" against nothing.
//!
//! This deliberately does not reuse [`cas_store::SqliteStore`], whose `open`
//! takes a read-write connection from the shared pool and runs `init()`
//! migrations. See the module docs on [`super`] for the full rationale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::ParityError;

/// How long to wait for a competing writer before giving up on a read.
const BUSY_TIMEOUT: Duration = Duration::from_millis(2_000);

/// One memory row, reduced to the fields retrieval parity cares about.
#[derive(Debug, Clone)]
pub struct MemoryRow {
    pub id: String,
    pub entry_type: String,
    pub memory_tier: String,
    pub title: Option<String>,
    pub content: String,
    pub tags: String,
}

/// A read-only handle on a project's `cas.db` memory table.
pub struct ReadOnlyMemoryDb {
    conn: Connection,
    db_path: PathBuf,
}

impl ReadOnlyMemoryDb {
    /// Open `<cas_dir>/cas.db` read-only.
    ///
    /// Errors if the database does not exist — a parity run against a database
    /// that isn't there is a bug, not an empty result set.
    pub fn open(cas_dir: &Path) -> Result<Self, ParityError> {
        let db_path = cas_dir.join("cas.db");
        if !db_path.exists() {
            return Err(ParityError::StoreUnavailable(format!(
                "no memory database at {}",
                db_path.display()
            )));
        }
        Self::open_db(&db_path)
    }

    /// Open a specific database file read-only.
    pub fn open_db(db_path: &Path) -> Result<Self, ParityError> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            ParityError::StoreUnavailable(format!(
                "cannot open {} read-only: {e}",
                db_path.display()
            ))
        })?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| ParityError::Sql(e.to_string()))?;
        Ok(Self {
            conn,
            db_path: db_path.to_path_buf(),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Attempt a raw statement on this connection.
    ///
    /// Exists so the read-only guarantee can be *asserted* rather than
    /// assumed: the test suite calls this with a `DELETE` and requires it to
    /// fail with `SQLITE_READONLY`. It is not `#[cfg(test)]` because the proof
    /// lives in an integration test, which compiles against the public API.
    /// Nothing in the harness itself calls it.
    pub fn exec_for_test(&self, sql: &str) -> Result<usize, rusqlite::Error> {
        self.conn.execute(sql, [])
    }

    /// The projection every query in this module shares, so that a row decoded
    /// by one channel is byte-identical to the same row decoded by another.
    const SELECT: &'static str =
        "SELECT id, type, memory_tier, title, content, COALESCE(tags, '') FROM entries";

    fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
        Ok(MemoryRow {
            id: row.get(0)?,
            entry_type: row.get(1)?,
            memory_tier: row.get(2)?,
            title: row.get(3)?,
            content: row.get(4)?,
            tags: row.get(5)?,
        })
    }

    fn query(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<MemoryRow>, ParityError> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| ParityError::Sql(format!("{e} (sql: {sql})")))?;
        let rows = stmt
            .query_map(params, Self::decode)
            .map_err(|e| ParityError::Sql(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            // No `.flatten()`: a row that fails to decode must surface as an
            // error, not vanish into a shorter result that reads as a
            // legitimate ranking.
            out.push(row.map_err(|e| ParityError::Sql(e.to_string()))?);
        }
        Ok(out)
    }

    /// Mirrors `SqliteStore::list`: non-archived, newest first.
    pub fn list(&self, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 ORDER BY created DESC, id ASC LIMIT ?1",
            Self::SELECT
        );
        self.query(&sql, &[&(limit as i64)])
    }

    /// Mirrors `SqliteStore::recent`: newest by created-or-updated.
    pub fn recent(&self, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 \
             ORDER BY MAX(created, COALESCE(updated_at, created)) DESC, id ASC LIMIT ?1",
            Self::SELECT
        );
        self.query(&sql, &[&(limit as i64)])
    }

    /// Mirrors `SqliteStore::list_pinned`: the in-context tier, which is what
    /// the SessionStart "Pinned Memories (Always Active)" block injects.
    pub fn pinned(&self, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 AND memory_tier = 'in-context' \
             ORDER BY created DESC, id ASC LIMIT ?1",
            Self::SELECT
        );
        self.query(&sql, &[&(limit as i64)])
    }

    /// Mirrors `SqliteStore::list_helpful`: feedback-ranked.
    pub fn helpful(&self, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 \
             ORDER BY (helpful_count - harmful_count) DESC, last_accessed DESC, id ASC LIMIT ?1",
            Self::SELECT
        );
        self.query(&sql, &[&(limit as i64)])
    }

    /// All non-archived entries of one [`cas_types::EntryType`].
    pub fn by_type(&self, entry_type: &str, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 AND lower(type) = lower(?1) \
             ORDER BY created DESC, id ASC LIMIT ?2",
            Self::SELECT
        );
        self.query(&sql, &[&entry_type, &(limit as i64)])
    }

    /// All non-archived entries in one [`cas_types::MemoryTier`].
    pub fn by_tier(&self, tier: &str, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 AND lower(memory_tier) = lower(?1) \
             ORDER BY created DESC, id ASC LIMIT ?2",
            Self::SELECT
        );
        self.query(&sql, &[&tier, &(limit as i64)])
    }

    /// Substring tag match, mirroring `store_list_by_scope_and_tag`.
    pub fn by_tag(&self, tag: &str, limit: usize) -> Result<Vec<MemoryRow>, ParityError> {
        let sql = format!(
            "{} WHERE archived = 0 AND instr(lower(COALESCE(tags, '')), lower(?1)) > 0 \
             ORDER BY created DESC, id ASC LIMIT ?2",
            Self::SELECT
        );
        self.query(&sql, &[&tag, &(limit as i64)])
    }

    /// Fetch specific ids (used to resolve search-index hits to content).
    pub fn get_many(&self, ids: &[String]) -> Result<BTreeMap<String, MemoryRow>, ParityError> {
        let mut out = BTreeMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let sql = format!("{} WHERE id = ?1", Self::SELECT);
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| ParityError::Sql(e.to_string()))?;
        for id in ids {
            let mut rows = stmt
                .query_map([id], Self::decode)
                .map_err(|e| ParityError::Sql(e.to_string()))?;
            if let Some(row) = rows.next() {
                let row = row.map_err(|e| ParityError::Sql(e.to_string()))?;
                out.insert(id.clone(), row);
            }
        }
        Ok(out)
    }

    /// Distinct non-archived entry types present in this corpus, lowercased.
    pub fn distinct_types(&self) -> Result<Vec<String>, ParityError> {
        self.distinct_column("type")
    }

    /// Distinct non-archived memory tiers present in this corpus, lowercased.
    pub fn distinct_tiers(&self) -> Result<Vec<String>, ParityError> {
        self.distinct_column("memory_tier")
    }

    fn distinct_column(&self, column: &str) -> Result<Vec<String>, ParityError> {
        // `column` is never caller-controlled — the only two call sites pass
        // hardcoded literals — so the format! here cannot carry injection.
        let sql =
            format!("SELECT DISTINCT lower({column}) FROM entries WHERE archived = 0 ORDER BY 1");
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| ParityError::Sql(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| ParityError::Sql(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| ParityError::Sql(e.to_string()))?);
        }
        Ok(out)
    }

    /// Count of non-archived entries, recorded in the baseline so a wildly
    /// different corpus size is visible in the report even when every
    /// individual query happens to pass.
    pub fn active_count(&self) -> Result<usize, ParityError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM entries WHERE archived = 0", [], |r| {
                r.get(0)
            })
            .map_err(|e| ParityError::Sql(e.to_string()))?;
        Ok(n as usize)
    }
}
