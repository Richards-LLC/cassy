//! Structural git-history index (EPIC cas-6212 / cas-7a21 + cas-0562, spec §4).
//!
//! Four tables, all project-scoped:
//!
//! - `history_commits` — one row per commit (subject/body/author/timestamps).
//! - `history_commit_files` — the structural diff mapping, one row per
//!   `(commit, file)` pair. Diffs are indexed *structurally*: which files a
//!   commit touched and how much, never the hunk text (spec §3, which makes
//!   this a privacy property as well as a cost one).
//! - `history_commit_symbols` — the symbol overlap (M3, spec §4.1): which
//!   symbols a commit's changed line ranges actually intersect. Written only
//!   where the symbol index has data; where it has none, the commit records
//!   [`SymbolMapping::Absent`] instead, so "not indexed yet" can never be read
//!   as "touched nothing" (spec §10.2).
//! - `history_index_state` — the watermark plus the honesty ledger, one row per
//!   `(repository, source)`.
//!
//! # Watermark contract
//!
//! [`SqliteHistoryStore::commit_batch`] writes commit rows, file rows and the
//! advanced watermark in **one** transaction. That is the whole reason the
//! method exists: a batch that half-lands must re-run, not be silently skipped.
//! The failure shape being avoided is cas-9d92's — two state fields that no
//! path reconciled, leaving rows both "done" and "not done" (spec §4.2 rule 2).
//!
//! Nothing here shells out to git; the walker owns that (`cas-cli/src/history`).
//! This module is storage only, so it stays testable without a repo on disk.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::shared_db;

/// Canonical DDL for the history subsystem, in `execute_batch` form.
pub const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_commits (
    sha TEXT PRIMARY KEY,
    short_sha TEXT NOT NULL,
    parent_shas TEXT NOT NULL DEFAULT '[]',
    is_merge INTEGER NOT NULL DEFAULT 0,
    author_name TEXT,
    author_email TEXT,
    authored_at TEXT,
    committed_at TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT,
    branch_hint TEXT,
    repository TEXT NOT NULL,
    pending_embedding INTEGER NOT NULL DEFAULT 1,
    indexed_at TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'project',
    symbol_mapping TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX IF NOT EXISTS idx_history_commits_committed_at
    ON history_commits(committed_at DESC);
CREATE INDEX IF NOT EXISTS idx_history_commits_short_sha
    ON history_commits(short_sha);
CREATE INDEX IF NOT EXISTS idx_history_commits_pending_embedding
    ON history_commits(committed_at) WHERE pending_embedding = 1;

CREATE TABLE IF NOT EXISTS history_commit_files (
    sha TEXT NOT NULL REFERENCES history_commits(sha) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    change_type TEXT NOT NULL,
    old_path TEXT,
    insertions INTEGER,
    deletions INTEGER,
    PRIMARY KEY (sha, file_path)
);

CREATE INDEX IF NOT EXISTS idx_history_commit_files_path
    ON history_commit_files(file_path);

CREATE TABLE IF NOT EXISTS history_index_state (
    repository TEXT NOT NULL,
    source TEXT NOT NULL,
    last_indexed_sha TEXT,
    last_indexed_at TEXT,
    last_attempt_at TEXT,
    last_error TEXT,
    backfill_complete INTEGER NOT NULL DEFAULT 0,
    items_indexed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repository, source)
);

CREATE TABLE IF NOT EXISTS history_commit_symbols (
    sha TEXT NOT NULL REFERENCES history_commits(sha) ON DELETE CASCADE,
    symbol_id TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    PRIMARY KEY (sha, symbol_id)
);

CREATE INDEX IF NOT EXISTS idx_history_commit_symbols_qualified_name
    ON history_commit_symbols(qualified_name);
"#;

/// Statement-level form of [`HISTORY_SCHEMA`] for the numbered migration
/// runner, which calls `Connection::execute` once per item.
///
/// Keep in lockstep with [`HISTORY_SCHEMA`]; the migration test compares the
/// resulting table/index shapes column-by-column and fails on any drift.
pub const HISTORY_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS history_commits (
        sha TEXT PRIMARY KEY,
        short_sha TEXT NOT NULL,
        parent_shas TEXT NOT NULL DEFAULT '[]',
        is_merge INTEGER NOT NULL DEFAULT 0,
        author_name TEXT,
        author_email TEXT,
        authored_at TEXT,
        committed_at TEXT NOT NULL,
        subject TEXT NOT NULL,
        body TEXT,
        branch_hint TEXT,
        repository TEXT NOT NULL,
        pending_embedding INTEGER NOT NULL DEFAULT 1,
        indexed_at TEXT NOT NULL,
        scope TEXT NOT NULL DEFAULT 'project',
        symbol_mapping TEXT NOT NULL DEFAULT 'pending'
    )",
    "CREATE INDEX IF NOT EXISTS idx_history_commits_committed_at
        ON history_commits(committed_at DESC)",
    "CREATE INDEX IF NOT EXISTS idx_history_commits_short_sha
        ON history_commits(short_sha)",
    "CREATE INDEX IF NOT EXISTS idx_history_commits_pending_embedding
        ON history_commits(committed_at) WHERE pending_embedding = 1",
    "CREATE TABLE IF NOT EXISTS history_commit_files (
        sha TEXT NOT NULL REFERENCES history_commits(sha) ON DELETE CASCADE,
        file_path TEXT NOT NULL,
        change_type TEXT NOT NULL,
        old_path TEXT,
        insertions INTEGER,
        deletions INTEGER,
        PRIMARY KEY (sha, file_path)
    )",
    "CREATE INDEX IF NOT EXISTS idx_history_commit_files_path
        ON history_commit_files(file_path)",
    "CREATE TABLE IF NOT EXISTS history_commit_symbols (
        sha TEXT NOT NULL REFERENCES history_commits(sha) ON DELETE CASCADE,
        symbol_id TEXT NOT NULL,
        qualified_name TEXT NOT NULL,
        file_path TEXT NOT NULL,
        PRIMARY KEY (sha, symbol_id)
    )",
    "CREATE INDEX IF NOT EXISTS idx_history_commit_symbols_qualified_name
        ON history_commit_symbols(qualified_name)",
    "CREATE TABLE IF NOT EXISTS history_index_state (
        repository TEXT NOT NULL,
        source TEXT NOT NULL,
        last_indexed_sha TEXT,
        last_indexed_at TEXT,
        last_attempt_at TEXT,
        last_error TEXT,
        backfill_complete INTEGER NOT NULL DEFAULT 0,
        items_indexed INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (repository, source)
    )",
];

/// The `history_index_state.source` value for the git walker.
pub const SOURCE_GIT: &str = "git";

/// One indexed commit.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryCommit {
    /// Full 40-char SHA.
    pub sha: String,
    /// Abbreviated SHA, stored at fixed width so §5.2's prefix joins have a
    /// stable left-hand side. Derived, never read back from `git --short`
    /// (whose width is dynamic — the cas-ea51 lesson).
    pub short_sha: String,
    pub parent_shas: Vec<String>,
    pub is_merge: bool,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub authored_at: Option<String>,
    pub committed_at: String,
    pub subject: String,
    pub body: Option<String>,
    pub branch_hint: Option<String>,
    pub repository: String,
}

/// One `(commit, file)` structural-diff row.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryCommitFile {
    pub sha: String,
    pub file_path: String,
    /// `A` | `M` | `D` | `R` | `C` | `T`.
    pub change_type: String,
    /// Populated for renames/copies only.
    pub old_path: Option<String>,
    /// `None` for binary files, where git reports `-` rather than a count.
    /// Deliberately not coerced to 0: "binary" and "no lines changed" are
    /// different facts and the index should not conflate them.
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
}

/// One `(commit, symbol)` overlap row — a symbol whose line range intersects a
/// line range the commit changed (spec §4.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryCommitSymbol {
    pub sha: String,
    /// `code_symbols.id`, so the row stays joinable to the live symbol index.
    pub symbol_id: String,
    pub qualified_name: String,
    /// Repo-relative path, matching `history_commit_files.file_path` rather
    /// than `code_symbols.file_path` (which is absolute — see the walker's
    /// bridging note). History rows stay portable across checkouts.
    pub file_path: String,
}

/// A symbol's line span as the symbol index currently holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRange {
    pub symbol_id: String,
    pub qualified_name: String,
    /// 1-based, inclusive, matching `code_symbols`.
    pub line_start: i64,
    pub line_end: i64,
}

/// What the symbol mapper was able to conclude for one commit.
///
/// The two values the spec names are [`Absent`](SymbolMapping::Absent) and
/// [`None_`](SymbolMapping::None_), and the distinction between them is the
/// whole point (spec §10.2): "the symbol index has no data for these files" and
/// "the symbol index has data and nothing overlapped" are different facts, and
/// collapsing them turns index lag into a silent empty result.
///
/// [`Partial`](SymbolMapping::Partial) and
/// [`NotApplicable`](SymbolMapping::NotApplicable) extend that pair rather than
/// replacing it, because a two-value enum forces two more lies: a commit that
/// touched ten files of which three are indexed has to claim one state for all
/// ten, and a docs-only commit would report `absent` — reading as index lag
/// when nothing about it was ever indexable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolMapping {
    /// Never attempted. The default for rows the M1 walker wrote.
    Pending,
    /// At least one overlap recorded; every eligible file was indexed.
    Mapped,
    /// Some eligible files were indexed, some were not. Overlaps for the
    /// indexed ones are recorded; the rest are honestly incomplete.
    Partial,
    /// Eligible files exist and **none** of them is in the symbol index. The
    /// spec §10.2 degradation state: answer "I cannot tell you", not "nothing".
    Absent,
    /// Every eligible file was indexed and nothing overlapped. A real,
    /// trustworthy zero — e.g. a change to an import block or a top-level
    /// comment that lies outside every symbol span.
    None_,
    /// The commit changed no indexable file at all (docs, config, binaries, or
    /// a merge commit with no first-parent diff). Not lag; nothing to map.
    NotApplicable,
}

impl SymbolMapping {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolMapping::Pending => "pending",
            SymbolMapping::Mapped => "mapped",
            SymbolMapping::Partial => "partial",
            SymbolMapping::Absent => "absent",
            SymbolMapping::None_ => "none",
            SymbolMapping::NotApplicable => "not_applicable",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => SymbolMapping::Pending,
            "mapped" => SymbolMapping::Mapped,
            "partial" => SymbolMapping::Partial,
            "absent" => SymbolMapping::Absent,
            "none" => SymbolMapping::None_,
            "not_applicable" => SymbolMapping::NotApplicable,
            _ => return None,
        })
    }

    /// Whether a later pass should retry this commit. `absent`/`partial` are
    /// *provisional* — they describe the symbol index's coverage at the moment
    /// of mapping, and M2 keeps catching up, so they must not be terminal.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            SymbolMapping::Pending | SymbolMapping::Absent | SymbolMapping::Partial
        )
    }
}

/// The watermark row plus its honesty fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryIndexState {
    pub repository: String,
    pub source: String,
    pub last_indexed_sha: Option<String>,
    pub last_indexed_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_error: Option<String>,
    pub backfill_complete: bool,
    pub items_indexed: i64,
}

/// Storage for the structural git-history index.
pub trait HistoryStore {
    /// Create tables if absent.
    fn init(&self) -> Result<()>;

    /// Write one chunk of commits, their file rows, and the advanced watermark
    /// in a single transaction (spec §4.2 rule 2).
    ///
    /// `watermark` is the SHA the pass reached; it becomes `last_indexed_sha`
    /// only if the whole chunk commits. Returns the number of commit rows
    /// written.
    fn commit_batch(
        &self,
        repository: &str,
        commits: &[HistoryCommit],
        files: &[HistoryCommitFile],
        watermark: &str,
        backfill_complete: bool,
    ) -> Result<usize>;

    /// Read the watermark row, if the source has ever run.
    fn index_state(&self, repository: &str, source: &str) -> Result<Option<HistoryIndexState>>;

    /// Record an attempt (and optionally its error) without touching the
    /// watermark. A failed pass must still be visible, or "never ran" and
    /// "ran and failed" become indistinguishable (spec §10.1).
    fn record_attempt(&self, repository: &str, source: &str, error: Option<&str>) -> Result<()>;

    /// Force a full re-backfill: clears the watermark and the completion flag.
    /// Used when the watermark is not an ancestor of HEAD (spec §4.2 rule 3).
    fn reset_watermark(&self, repository: &str, source: &str) -> Result<()>;

    /// `(commits, commit-file pairs)` currently indexed for a repository.
    fn counts(&self, repository: &str) -> Result<(i64, i64)>;

    /// Whether a commit is already indexed (used by the delta walker's tests
    /// and by `cas history status`).
    fn has_commit(&self, sha: &str) -> Result<bool>;

    // ---- M3: symbol mapping (cas-0562, spec §4.1) ----

    /// Commits whose symbol mapping is still worth attempting, oldest first.
    ///
    /// Returns `pending`, `absent` and `partial` commits — see
    /// [`SymbolMapping::is_retryable`] for why the latter two are not terminal.
    fn commits_awaiting_symbol_mapping(
        &self,
        repository: &str,
        limit: usize,
    ) -> Result<Vec<String>>;

    /// The symbol index's view of one file, keyed the way **M2** keys it:
    /// `repo_name` is the repository *directory name* and `abs_path` is an
    /// absolute path (`daemon/indexing.rs`). The history tables key on the repo
    /// *root path* and repo-*relative* file paths, so the walker bridges.
    ///
    /// `Ok(None)` means the symbol index has never seen the file — the
    /// [`SymbolMapping::Absent`] signal. `Ok(Some(vec![]))` means it parsed the
    /// file and found no symbols, which is a genuine zero and must not be
    /// reported as lag.
    fn symbol_ranges_for_file(
        &self,
        repo_name: &str,
        abs_path: &str,
    ) -> Result<Option<Vec<SymbolRange>>>;

    /// Write the overlap rows for a batch of commits and stamp each commit's
    /// `symbol_mapping`, in **one** transaction.
    ///
    /// Same contract as [`HistoryStore::commit_batch`] and for the same reason:
    /// a commit stamped `mapped` whose symbol rows did not land would be a row
    /// that is both "mapped" and empty, with nothing to reconcile it. Existing
    /// rows for each listed commit are cleared first, so a re-map after the
    /// symbol index catches up replaces rather than accumulates.
    fn record_symbol_mapping(
        &self,
        mappings: &[(String, SymbolMapping)],
        symbols: &[HistoryCommitSymbol],
    ) -> Result<usize>;

    /// `symbol_mapping` value → commit count, for a repository. The honesty
    /// surface: `absent` being large is exactly the thing that must be visible.
    fn symbol_mapping_counts(&self, repository: &str) -> Result<Vec<(String, i64)>>;

    /// Overlap rows recorded for one commit.
    fn symbols_for_commit(&self, sha: &str) -> Result<Vec<HistoryCommitSymbol>>;
}

/// SQLite-backed [`HistoryStore`] sharing the process-wide `cas.db` connection.
pub struct SqliteHistoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteHistoryStore {
    /// Open (and initialize) the history store rooted at `cas_dir`.
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Wrap an existing shared connection (daemon path — avoids a second open).
    pub fn from_connection(conn: Arc<Mutex<Connection>>) -> Result<Self> {
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn state_from_row(row: &rusqlite::Row) -> rusqlite::Result<HistoryIndexState> {
        Ok(HistoryIndexState {
            repository: row.get("repository")?,
            source: row.get("source")?,
            last_indexed_sha: row.get("last_indexed_sha")?,
            last_indexed_at: row.get("last_indexed_at")?,
            last_attempt_at: row.get("last_attempt_at")?,
            last_error: row.get("last_error")?,
            backfill_complete: row.get::<_, i64>("backfill_complete")? != 0,
            items_indexed: row.get("items_indexed")?,
        })
    }
}

impl HistoryStore for SqliteHistoryStore {
    fn init(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(HISTORY_SCHEMA)?;

        // `CREATE TABLE IF NOT EXISTS` is a no-op on a store that M1 already
        // created, so M3's added column needs its own idempotent ALTER. The
        // numbered migration (m222) does this too; doing it here as well keeps
        // the store self-sufficient for any path that opens it before the
        // migration runner has had a turn, rather than failing on a missing
        // column at query time.
        let has_column = conn
            .prepare("SELECT 1 FROM pragma_table_info('history_commits') WHERE name = 'symbol_mapping'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(
                "ALTER TABLE history_commits
                     ADD COLUMN symbol_mapping TEXT NOT NULL DEFAULT 'pending'",
            )?;
        }
        Ok(())
    }

    fn commit_batch(
        &self,
        repository: &str,
        commits: &[HistoryCommit],
        files: &[HistoryCommitFile],
        watermark: &str,
        backfill_complete: bool,
    ) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO history_commits (
                     sha, short_sha, parent_shas, is_merge, author_name, author_email,
                     authored_at, committed_at, subject, body, branch_hint, repository,
                     pending_embedding, indexed_at, scope
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, 'project')
                 ON CONFLICT(sha) DO UPDATE SET
                     short_sha = excluded.short_sha,
                     parent_shas = excluded.parent_shas,
                     is_merge = excluded.is_merge,
                     author_name = excluded.author_name,
                     author_email = excluded.author_email,
                     authored_at = excluded.authored_at,
                     committed_at = excluded.committed_at,
                     subject = excluded.subject,
                     body = excluded.body,
                     branch_hint = excluded.branch_hint,
                     repository = excluded.repository,
                     indexed_at = excluded.indexed_at",
            )?;
            for c in commits {
                let parents = serde_json::to_string(&c.parent_shas).unwrap_or_else(|_| "[]".into());
                stmt.execute(params![
                    c.sha,
                    c.short_sha,
                    parents,
                    i64::from(c.is_merge),
                    c.author_name,
                    c.author_email,
                    c.authored_at,
                    c.committed_at,
                    c.subject,
                    c.body,
                    c.branch_hint,
                    c.repository,
                    now,
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO history_commit_files (
                     sha, file_path, change_type, old_path, insertions, deletions
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(sha, file_path) DO UPDATE SET
                     change_type = excluded.change_type,
                     old_path = excluded.old_path,
                     insertions = excluded.insertions,
                     deletions = excluded.deletions",
            )?;
            for f in files {
                stmt.execute(params![
                    f.sha,
                    f.file_path,
                    f.change_type,
                    f.old_path,
                    f.insertions,
                    f.deletions,
                ])?;
            }
        }

        // The watermark advances in the SAME transaction as the rows it
        // describes. `items_indexed` accumulates rather than being recomputed,
        // so a chunked backfill reports honest running progress.
        tx.execute(
            "INSERT INTO history_index_state (
                 repository, source, last_indexed_sha, last_indexed_at, last_attempt_at,
                 last_error, backfill_complete, items_indexed
             ) VALUES (?1, ?2, ?3, ?4, ?4, NULL, ?5, ?6)
             ON CONFLICT(repository, source) DO UPDATE SET
                 last_indexed_sha = excluded.last_indexed_sha,
                 last_indexed_at = excluded.last_indexed_at,
                 last_attempt_at = excluded.last_attempt_at,
                 last_error = NULL,
                 backfill_complete = excluded.backfill_complete,
                 items_indexed = history_index_state.items_indexed + excluded.items_indexed",
            params![
                repository,
                SOURCE_GIT,
                watermark,
                now,
                i64::from(backfill_complete),
                commits.len() as i64,
            ],
        )?;

        tx.commit()?;
        Ok(commits.len())
    }

    fn index_state(&self, repository: &str, source: &str) -> Result<Option<HistoryIndexState>> {
        let conn = self.lock();
        let state = conn
            .query_row(
                "SELECT * FROM history_index_state WHERE repository = ?1 AND source = ?2",
                params![repository, source],
                Self::state_from_row,
            )
            .optional()?;
        Ok(state)
    }

    fn record_attempt(&self, repository: &str, source: &str, error: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO history_index_state (
                 repository, source, last_attempt_at, last_error
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(repository, source) DO UPDATE SET
                 last_attempt_at = excluded.last_attempt_at,
                 last_error = excluded.last_error",
            params![repository, source, now, error],
        )?;
        Ok(())
    }

    fn reset_watermark(&self, repository: &str, source: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE history_index_state
                SET last_indexed_sha = NULL, backfill_complete = 0
              WHERE repository = ?1 AND source = ?2",
            params![repository, source],
        )?;
        Ok(())
    }

    fn counts(&self, repository: &str) -> Result<(i64, i64)> {
        let conn = self.lock();
        let commits: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_commits WHERE repository = ?1",
            params![repository],
            |row| row.get(0),
        )?;
        let pairs: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_commit_files f
               JOIN history_commits c ON c.sha = f.sha
              WHERE c.repository = ?1",
            params![repository],
            |row| row.get(0),
        )?;
        Ok((commits, pairs))
    }

    fn has_commit(&self, sha: &str) -> Result<bool> {
        let conn = self.lock();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM history_commits WHERE sha = ?1)",
            params![sha],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    fn commits_awaiting_symbol_mapping(
        &self,
        repository: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let conn = self.lock();
        // Oldest first, matching the walker's topological order, so a bounded
        // pass makes monotonic progress instead of re-chewing the same tail.
        let mut stmt = conn.prepare(
            "SELECT sha FROM history_commits
              WHERE repository = ?1
                AND symbol_mapping IN ('pending', 'absent', 'partial')
              ORDER BY committed_at ASC
              LIMIT ?2",
        )?;
        let shas = stmt
            .query_map(params![repository, limit as i64], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(shas)
    }

    fn symbol_ranges_for_file(
        &self,
        repo_name: &str,
        abs_path: &str,
    ) -> Result<Option<Vec<SymbolRange>>> {
        let conn = self.lock();

        // The symbol index is a *separate* subsystem (M2) that may never have
        // been initialised in this store. A missing table is the strongest
        // possible form of "no data for this file", so it degrades to the same
        // Absent signal rather than erroring the whole mapping pass.
        let code_tables: bool = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'code_files'
             )",
            [],
            |row| row.get(0),
        )?;
        if !code_tables {
            return Ok(None);
        }

        // `SqliteCodeStore` normalizes on write — an absolute path loses its
        // leading `/` before it is stored — so the probe must be normalized
        // through the SAME function, not merely "similarly". Comparing the raw
        // path here matched nothing at all and reported every file as absent,
        // which the honesty rule would then have dutifully reported as index
        // lag forever.
        let abs_path = crate::SqliteCodeStore::normalize_path(abs_path);

        // Presence in `code_files` — not in `code_symbols` — is the "M2 has
        // seen this file" signal. Keying absence on `code_symbols` (as spec
        // §4.1 words it) would report a file that genuinely parsed to zero
        // symbols as permanently un-indexed, i.e. as lag that never clears.
        let seen: bool = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM code_files WHERE repository = ?1 AND path = ?2
             )",
            params![repo_name, abs_path],
            |row| row.get(0),
        )?;
        if !seen {
            return Ok(None);
        }

        let mut stmt = conn.prepare(
            "SELECT id, qualified_name, line_start, line_end
               FROM code_symbols
              WHERE repository = ?1 AND file_path = ?2",
        )?;
        let ranges = stmt
            .query_map(params![repo_name, abs_path], |row| {
                Ok(SymbolRange {
                    symbol_id: row.get(0)?,
                    qualified_name: row.get(1)?,
                    line_start: row.get(2)?,
                    line_end: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(ranges))
    }

    fn record_symbol_mapping(
        &self,
        mappings: &[(String, SymbolMapping)],
        symbols: &[HistoryCommitSymbol],
    ) -> Result<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        {
            let mut clear = tx.prepare("DELETE FROM history_commit_symbols WHERE sha = ?1")?;
            let mut stamp =
                tx.prepare("UPDATE history_commits SET symbol_mapping = ?2 WHERE sha = ?1")?;
            for (sha, mapping) in mappings {
                clear.execute(params![sha])?;
                stamp.execute(params![sha, mapping.as_str()])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO history_commit_symbols (sha, symbol_id, qualified_name, file_path)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(sha, symbol_id) DO UPDATE SET
                     qualified_name = excluded.qualified_name,
                     file_path = excluded.file_path",
            )?;
            for s in symbols {
                stmt.execute(params![s.sha, s.symbol_id, s.qualified_name, s.file_path])?;
            }
        }

        tx.commit()?;
        Ok(symbols.len())
    }

    fn symbol_mapping_counts(&self, repository: &str) -> Result<Vec<(String, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT symbol_mapping, COUNT(*) FROM history_commits
              WHERE repository = ?1
              GROUP BY symbol_mapping
              ORDER BY symbol_mapping",
        )?;
        let counts = stmt
            .query_map(params![repository], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(counts)
    }

    fn symbols_for_commit(&self, sha: &str) -> Result<Vec<HistoryCommitSymbol>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT sha, symbol_id, qualified_name, file_path
               FROM history_commit_symbols
              WHERE sha = ?1
              ORDER BY qualified_name",
        )?;
        let rows = stmt
            .query_map(params![sha], |row| {
                Ok(HistoryCommitSymbol {
                    sha: row.get(0)?,
                    symbol_id: row.get(1)?,
                    qualified_name: row.get(2)?,
                    file_path: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, SqliteHistoryStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteHistoryStore::open(temp.path()).unwrap();
        (temp, store)
    }

    fn commit(sha: &str) -> HistoryCommit {
        HistoryCommit {
            sha: sha.to_string(),
            short_sha: sha.chars().take(8).collect(),
            parent_shas: vec!["p".repeat(40)],
            is_merge: false,
            author_name: Some("Ada".into()),
            author_email: Some("ada@example.com".into()),
            authored_at: Some("2026-08-08T00:00:00Z".into()),
            committed_at: "2026-08-08T00:00:00Z".into(),
            subject: format!("subject {sha}"),
            body: Some("body".into()),
            branch_hint: Some("main".into()),
            repository: "/repo".into(),
        }
    }

    fn file_row(sha: &str, path: &str) -> HistoryCommitFile {
        HistoryCommitFile {
            sha: sha.to_string(),
            file_path: path.to_string(),
            change_type: "M".into(),
            old_path: None,
            insertions: Some(3),
            deletions: Some(1),
        }
    }

    #[test]
    fn commit_batch_writes_rows_and_advances_watermark() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[commit(&a)],
                &[file_row(&a, "src/lib.rs")],
                &a,
                true,
            )
            .unwrap();

        assert_eq!(store.counts("/repo").unwrap(), (1, 1));
        let state = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert_eq!(state.last_indexed_sha.as_deref(), Some(a.as_str()));
        assert!(state.backfill_complete);
        assert_eq!(state.items_indexed, 1);
        assert!(store.has_commit(&a).unwrap());
    }

    #[test]
    fn items_indexed_accumulates_across_chunks() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, false)
            .unwrap();
        store
            .commit_batch("/repo", &[commit(&b)], &[], &b, true)
            .unwrap();

        let state = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert_eq!(state.items_indexed, 2);
        assert_eq!(state.last_indexed_sha.as_deref(), Some(b.as_str()));
        assert!(state.backfill_complete);
    }

    /// Re-indexing the same commit must not duplicate file rows — the delta
    /// walker can legitimately re-run a chunk after a failure.
    #[test]
    fn reindexing_a_commit_is_idempotent() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        for _ in 0..2 {
            store
                .commit_batch(
                    "/repo",
                    &[commit(&a)],
                    &[file_row(&a, "src/lib.rs")],
                    &a,
                    true,
                )
                .unwrap();
        }
        assert_eq!(store.counts("/repo").unwrap(), (1, 1));
    }

    /// The transactional guarantee that replaces cas-9d92's split state: if the
    /// row write fails, the watermark must not move.
    #[test]
    fn failed_batch_leaves_watermark_untouched() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, false)
            .unwrap();

        // A file row whose parent commit does not exist violates the FK.
        {
            let conn = store.lock();
            conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        }
        let orphan = file_row(&"z".repeat(40), "src/nope.rs");
        let err = store.commit_batch("/repo", &[], &[orphan], &"b".repeat(40), true);
        assert!(err.is_err(), "orphan file row must fail the batch");

        let state = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert_eq!(
            state.last_indexed_sha.as_deref(),
            Some(a.as_str()),
            "watermark advanced despite a failed batch"
        );
        assert!(!state.backfill_complete);
    }

    #[test]
    fn record_attempt_sets_error_without_moving_watermark() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();
        store
            .record_attempt("/repo", SOURCE_GIT, Some("git exploded"))
            .unwrap();

        let state = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert_eq!(state.last_error.as_deref(), Some("git exploded"));
        assert_eq!(state.last_indexed_sha.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn a_successful_batch_clears_a_previous_error() {
        let (_t, store) = store();
        store
            .record_attempt("/repo", SOURCE_GIT, Some("transient"))
            .unwrap();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();
        let state = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert!(state.last_error.is_none());
    }

    #[test]
    fn reset_watermark_forces_a_rebackfill() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();
        store.reset_watermark("/repo", SOURCE_GIT).unwrap();

        let state = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert!(state.last_indexed_sha.is_none());
        assert!(!state.backfill_complete);
        // Rows survive: the re-backfill upserts over them rather than losing
        // the index while it re-runs.
        assert_eq!(store.counts("/repo").unwrap().0, 1);
    }

    #[test]
    fn counts_are_per_repository() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let mut other = commit(&"b".repeat(40));
        other.repository = "/elsewhere".into();
        store
            .commit_batch(
                "/repo",
                &[commit(&a)],
                &[file_row(&a, "src/lib.rs")],
                &a,
                true,
            )
            .unwrap();
        store
            .commit_batch("/elsewhere", &[other], &[], &"b".repeat(40), true)
            .unwrap();

        assert_eq!(store.counts("/repo").unwrap(), (1, 1));
        assert_eq!(store.counts("/elsewhere").unwrap(), (1, 0));
    }

    #[test]
    fn index_state_is_none_before_any_run() {
        let (_t, store) = store();
        assert!(store.index_state("/repo", SOURCE_GIT).unwrap().is_none());
    }

    #[test]
    fn binary_files_keep_null_line_counts() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let binary = HistoryCommitFile {
            sha: a.clone(),
            file_path: "logo.png".into(),
            change_type: "A".into(),
            old_path: None,
            insertions: None,
            deletions: None,
        };
        store
            .commit_batch("/repo", &[commit(&a)], &[binary], &a, true)
            .unwrap();

        let conn = store.lock();
        let ins: Option<i64> = conn
            .query_row(
                "SELECT insertions FROM history_commit_files WHERE file_path = 'logo.png'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(ins.is_none(), "binary line counts must stay NULL, not 0");
    }

    // ---- M3: symbol mapping (cas-0562) ----

    /// Create the two M2 tables this store reads through, so the symbol-mapping
    /// tests exercise the real join rather than a stand-in.
    fn with_code_tables(store: &SqliteHistoryStore) {
        let conn = store.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS code_files (
                 id TEXT PRIMARY KEY, path TEXT NOT NULL, repository TEXT NOT NULL,
                 language TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0,
                 line_count INTEGER NOT NULL DEFAULT 0, commit_hash TEXT,
                 content_hash TEXT NOT NULL, created TEXT NOT NULL, updated TEXT NOT NULL,
                 scope TEXT NOT NULL DEFAULT 'project', UNIQUE(repository, path)
             );
             CREATE TABLE IF NOT EXISTS code_symbols (
                 id TEXT PRIMARY KEY, qualified_name TEXT NOT NULL, name TEXT NOT NULL,
                 kind TEXT NOT NULL, language TEXT NOT NULL, file_path TEXT NOT NULL,
                 file_id TEXT NOT NULL, line_start INTEGER NOT NULL, line_end INTEGER NOT NULL,
                 source TEXT NOT NULL, documentation TEXT, signature TEXT, parent_id TEXT,
                 repository TEXT NOT NULL, created TEXT NOT NULL, updated TEXT NOT NULL,
                 commit_hash TEXT, content_hash TEXT NOT NULL, scope TEXT NOT NULL DEFAULT 'project'
             );",
        )
        .unwrap();
    }

    /// Seeds the *normalized* form, because that is what `SqliteCodeStore`
    /// writes. Seeding the raw path would make these tests pass against a
    /// lookup that cannot work in production.
    fn seed_code_file(store: &SqliteHistoryStore, repo: &str, path: &str) {
        let path = &crate::SqliteCodeStore::normalize_path(path);
        let conn = store.lock();
        conn.execute(
            "INSERT INTO code_files (id, path, repository, language, content_hash, created, updated)
             VALUES (?1, ?2, ?3, 'rust', 'h', 'now', 'now')",
            params![format!("{repo}:{path}"), path, repo],
        )
        .unwrap();
    }

    fn seed_symbol(
        store: &SqliteHistoryStore,
        repo: &str,
        path: &str,
        id: &str,
        name: &str,
        start: i64,
        end: i64,
    ) {
        let path = &crate::SqliteCodeStore::normalize_path(path);
        let conn = store.lock();
        conn.execute(
            "INSERT INTO code_symbols (
                 id, qualified_name, name, kind, language, file_path, file_id,
                 line_start, line_end, source, repository, created, updated, content_hash
             ) VALUES (?1, ?2, ?2, 'function', 'rust', ?3, 'f', ?4, ?5, 'src', ?6, 'now', 'now', 'h')",
            params![id, name, path, start, end, repo],
        )
        .unwrap();
    }

    /// The whole symbol subsystem may not exist in a store. That is the
    /// strongest form of "no data", and must degrade to Absent rather than
    /// erroring out the mapping pass.
    #[test]
    fn symbol_ranges_are_absent_when_the_code_tables_do_not_exist() {
        let (_t, store) = store();
        assert_eq!(store.symbol_ranges_for_file("repo", "/repo/a.rs").unwrap(), None);
    }

    #[test]
    fn symbol_ranges_are_absent_for_a_file_the_index_has_not_seen() {
        let (_t, store) = store();
        with_code_tables(&store);
        seed_code_file(&store, "repo", "/repo/known.rs");
        assert_eq!(
            store.symbol_ranges_for_file("repo", "/repo/unknown.rs").unwrap(),
            None
        );
    }

    /// The refinement over spec §4.1's wording: a parsed file with zero symbols
    /// is *covered*, and reporting it as absent would be permanent false lag.
    #[test]
    fn an_indexed_file_with_no_symbols_reads_as_covered_not_absent() {
        let (_t, store) = store();
        with_code_tables(&store);
        seed_code_file(&store, "repo", "/repo/empty.rs");
        assert_eq!(
            store.symbol_ranges_for_file("repo", "/repo/empty.rs").unwrap(),
            Some(Vec::new())
        );
    }

    #[test]
    fn symbol_ranges_are_scoped_to_the_repository() {
        let (_t, store) = store();
        with_code_tables(&store);
        seed_code_file(&store, "repo", "/repo/a.rs");
        seed_symbol(&store, "repo", "/repo/a.rs", "s1", "a::one", 1, 5);
        seed_code_file(&store, "other", "/other/a.rs");
        seed_symbol(&store, "other", "/other/a.rs", "s2", "a::two", 1, 5);

        let ranges = store.symbol_ranges_for_file("repo", "/repo/a.rs").unwrap().unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].qualified_name, "a::one");
    }

    #[test]
    fn record_symbol_mapping_writes_rows_and_stamps_the_verdict() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store.commit_batch("/repo", &[commit(&a)], &[], &a, true).unwrap();

        let rows = vec![HistoryCommitSymbol {
            sha: a.clone(),
            symbol_id: "s1".into(),
            qualified_name: "lib::alpha".into(),
            file_path: "src/lib.rs".into(),
        }];
        store
            .record_symbol_mapping(&[(a.clone(), SymbolMapping::Mapped)], &rows)
            .unwrap();

        let stored = store.symbols_for_commit(&a).unwrap();
        assert_eq!(stored, rows);
        assert_eq!(
            store.symbol_mapping_counts("/repo").unwrap(),
            vec![("mapped".to_string(), 1)]
        );
    }

    /// Re-mapping after the symbol index catches up must REPLACE the previous
    /// answer. Accumulating would leave a commit carrying symbols from a stale
    /// parse alongside the current ones, with nothing to tell them apart.
    #[test]
    fn remapping_replaces_the_previous_answer() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store.commit_batch("/repo", &[commit(&a)], &[], &a, true).unwrap();

        store
            .record_symbol_mapping(
                &[(a.clone(), SymbolMapping::Mapped)],
                &[HistoryCommitSymbol {
                    sha: a.clone(),
                    symbol_id: "stale".into(),
                    qualified_name: "lib::gone".into(),
                    file_path: "src/lib.rs".into(),
                }],
            )
            .unwrap();
        store
            .record_symbol_mapping(
                &[(a.clone(), SymbolMapping::Mapped)],
                &[HistoryCommitSymbol {
                    sha: a.clone(),
                    symbol_id: "fresh".into(),
                    qualified_name: "lib::current".into(),
                    file_path: "src/lib.rs".into(),
                }],
            )
            .unwrap();

        let stored = store.symbols_for_commit(&a).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].symbol_id, "fresh");
    }

    /// `absent` and `partial` are provisional — the symbol index keeps catching
    /// up — so they must come back for another pass. `mapped` and `none` are
    /// settled and must not, or every pass would re-chew the whole corpus.
    #[test]
    fn only_retryable_verdicts_come_back_for_another_pass() {
        let (_t, store) = store();
        let shas: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|c| c.repeat(40))
            .collect();
        for sha in &shas {
            store.commit_batch("/repo", &[commit(sha)], &[], sha, true).unwrap();
        }
        store
            .record_symbol_mapping(
                &[
                    (shas[0].clone(), SymbolMapping::Mapped),
                    (shas[1].clone(), SymbolMapping::None_),
                    (shas[2].clone(), SymbolMapping::Absent),
                    (shas[3].clone(), SymbolMapping::Partial),
                    (shas[4].clone(), SymbolMapping::NotApplicable),
                ],
                &[],
            )
            .unwrap();

        let awaiting = store.commits_awaiting_symbol_mapping("/repo", 100).unwrap();
        assert_eq!(awaiting.len(), 2);
        assert!(awaiting.contains(&shas[2]));
        assert!(awaiting.contains(&shas[3]));
    }

    /// Commits M1 wrote before M3 existed must be queued, not silently treated
    /// as "already mapped, found nothing".
    #[test]
    fn commits_default_to_pending_and_are_queued() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store.commit_batch("/repo", &[commit(&a)], &[], &a, true).unwrap();
        assert_eq!(
            store.symbol_mapping_counts("/repo").unwrap(),
            vec![("pending".to_string(), 1)]
        );
        assert_eq!(
            store.commits_awaiting_symbol_mapping("/repo", 100).unwrap(),
            vec![a]
        );
    }

    /// A delta pass legitimately re-upserts a commit it has already seen. That
    /// must not discard a mapping verdict and send the commit round again.
    #[test]
    fn reindexing_a_commit_preserves_its_mapping_verdict() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store.commit_batch("/repo", &[commit(&a)], &[], &a, true).unwrap();
        store
            .record_symbol_mapping(&[(a.clone(), SymbolMapping::None_)], &[])
            .unwrap();

        store.commit_batch("/repo", &[commit(&a)], &[], &a, true).unwrap();

        assert_eq!(
            store.symbol_mapping_counts("/repo").unwrap(),
            vec![("none".to_string(), 1)]
        );
    }

    #[test]
    fn awaiting_respects_the_limit_and_repository_scope() {
        let (_t, store) = store();
        for c in ["a", "b", "c"] {
            let sha = c.repeat(40);
            store.commit_batch("/repo", &[commit(&sha)], &[], &sha, true).unwrap();
        }
        let mut elsewhere = commit(&"z".repeat(40));
        elsewhere.repository = "/elsewhere".into();
        store
            .commit_batch("/elsewhere", &[elsewhere], &[], &"z".repeat(40), true)
            .unwrap();

        assert_eq!(store.commits_awaiting_symbol_mapping("/repo", 2).unwrap().len(), 2);
        assert_eq!(
            store.commits_awaiting_symbol_mapping("/elsewhere", 10).unwrap().len(),
            1
        );
    }

    /// The join that silently matched nothing until `normalize_path` was
    /// applied to the probe: callers hold an absolute path, the store holds it
    /// without its leading slash.
    #[test]
    fn an_absolute_probe_matches_the_normalized_stored_path() {
        let (_t, store) = store();
        with_code_tables(&store);

        // Written exactly as `SqliteCodeStore::add_file` would write it.
        {
            let conn = store.lock();
            conn.execute(
                "INSERT INTO code_files (id, path, repository, language, content_hash, created, updated)
                 VALUES ('f', 'repo/src/lib.rs', 'repo', 'rust', 'h', 'now', 'now')",
                [],
            )
            .unwrap();
        }

        assert!(
            store
                .symbol_ranges_for_file("repo", "/repo/src/lib.rs")
                .unwrap()
                .is_some(),
            "an absolute probe must resolve against the normalized stored path"
        );
    }

    /// Every enum value must survive a round trip through the column, or a
    /// verdict written by one release becomes unreadable to the next.
    #[test]
    fn every_mapping_value_round_trips_through_its_string_form() {
        for mapping in [
            SymbolMapping::Pending,
            SymbolMapping::Mapped,
            SymbolMapping::Partial,
            SymbolMapping::Absent,
            SymbolMapping::None_,
            SymbolMapping::NotApplicable,
        ] {
            assert_eq!(SymbolMapping::from_str(mapping.as_str()), Some(mapping));
        }
        assert_eq!(SymbolMapping::from_str("nonsense"), None);
    }

    #[test]
    fn rename_rows_keep_old_path() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let renamed = HistoryCommitFile {
            sha: a.clone(),
            file_path: "new/path.rs".into(),
            change_type: "R".into(),
            old_path: Some("old/path.rs".into()),
            insertions: Some(0),
            deletions: Some(0),
        };
        store
            .commit_batch("/repo", &[commit(&a)], &[renamed.clone()], &a, true)
            .unwrap();

        let conn = store.lock();
        let old: Option<String> = conn
            .query_row(
                "SELECT old_path FROM history_commit_files WHERE file_path = 'new/path.rs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old.as_deref(), Some("old/path.rs"));
    }
}
