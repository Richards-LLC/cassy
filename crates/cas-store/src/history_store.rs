//! Structural git-history index (EPIC cas-6212 / cas-7a21, spec §4).
//!
//! Three tables, all project-scoped:
//!
//! - `history_commits` — one row per commit (subject/body/author/timestamps).
//! - `history_commit_files` — the structural diff mapping, one row per
//!   `(commit, file)` pair. Diffs are indexed *structurally*: which files a
//!   commit touched and how much, never the hunk text (spec §3, which makes
//!   this a privacy property as well as a cost one).
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
    scope TEXT NOT NULL DEFAULT 'project'
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

CREATE VIRTUAL TABLE IF NOT EXISTS history_commits_fts USING fts5(
    sha UNINDEXED,
    subject,
    body
);
"#;

/// The FTS5 index over commit prose, plus a one-time backfill for databases
/// whose `history_commits` rows predate it (EPIC cas-6212 / cas-7f40, M4).
///
/// Split out of [`HISTORY_SCHEMA_STATEMENTS`] rather than appended to it
/// because m221 has already run on live databases: its `detect` predicate would
/// short-circuit and the FTS table would never appear. A separate migration is
/// the only shape that reaches those installs.
///
/// **Not contentless.** `knowledge_pages_fts` uses `content=''` and joins back
/// on `rowid` because it has a stable `row_id INTEGER PRIMARY KEY AUTOINCREMENT`
/// to join to. `history_commits` is keyed by `sha TEXT PRIMARY KEY`, so its
/// rowid is implicit — and SQLite does not preserve implicit rowids across
/// `VACUUM`, which would silently disconnect every FTS row from its commit.
/// Storing `sha` in the index costs ~1.7 MB of duplicated prose on this repo
/// (against a 456 MB database) and removes the coupling entirely.
pub const HISTORY_FTS_STATEMENTS: &[&str] = &[
    "CREATE VIRTUAL TABLE IF NOT EXISTS history_commits_fts USING fts5(
        sha UNINDEXED,
        subject,
        body
    )",
    // Backfill is guarded rather than unconditional: re-running it on a
    // populated index would double every commit's terms and quietly skew bm25().
    "INSERT INTO history_commits_fts (sha, subject, body)
     SELECT sha, subject, COALESCE(body, '') FROM history_commits
      WHERE NOT EXISTS (SELECT 1 FROM history_commits_fts)",
];

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
        scope TEXT NOT NULL DEFAULT 'project'
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

/// Filters for a history query (spec §6.1).
///
/// Every field is a *narrowing* filter except `text`, which is the lexical
/// recall term. `text: None` is a legitimate query — it is what Q2 and Q3 are:
/// "everything that touched this path in this window", ordered by recency.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct HistoryQuery {
    /// Repository identity (see `cas-cli`'s `history::repository_id`).
    pub repository: String,
    /// Free text matched against commit subject + body via FTS5.
    pub text: Option<String>,
    /// Substring match against `history_commit_files.file_path`. A commit
    /// matches when *any* of its touched paths contains this.
    pub path: Option<String>,
    /// Inclusive lower bound on `committed_at` (RFC3339).
    pub since: Option<String>,
    /// Inclusive upper bound on `committed_at` (RFC3339).
    pub until: Option<String>,
    /// Merge commits are indexed structurally but carry `Merge branch 'x'` as
    /// their whole message (spec §7.1), so they are noise in a text search.
    /// Off by default; callers asking a structural question can turn them on.
    pub include_merges: bool,
    pub limit: usize,
}

/// One ranked commit, with the file rows that made it match.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryCommitHit {
    pub commit: HistoryCommit,
    /// Lexical relevance (larger = better) when `text` was supplied; the
    /// `0.5^(days/30)` recency decay when it was not. **Never a blend of the
    /// two** — a five-month-old exact match would otherwise score below a
    /// yesterday commit that matched nothing, which is not what "search for
    /// this text" means.
    pub score: f64,
    /// `0.5^(days/30)`, always reported so a caller can re-rank on recency
    /// without re-deriving it (spec §6.2's recency term).
    pub recency: f64,
    /// The commit's structural diff rows, narrowed to `path` when one was given.
    pub files: Vec<HistoryCommitFile>,
}

/// One co-change row: a file that tends to change alongside the queried one
/// (spec §6.4 Q7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoChangedFile {
    pub file_path: String,
    /// Commits touching both this file and the queried one.
    pub commits_together: i64,
}

/// Measured provenance reach, split by confidence (spec §10.1).
///
/// This type deliberately cannot express "provenance works". It reports how
/// many commits carry the one high-confidence edge that exists today and
/// nothing else, because until M5 nothing else does exist.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvenanceCoverage {
    /// Commits indexed for the repository — the denominator.
    pub total_commits: i64,
    /// Commits reachable through `tasks.deliverables.factory_branch_anchor`,
    /// the only exact, unambiguous commit→task edge that is populated
    /// (spec §5.2). `None` when the edge cannot be measured at all.
    pub high_confidence_linked: Option<i64>,
    /// `high_confidence_linked / total_commits * 100`, `None` when either side
    /// is unknown. Never defaulted to 0.0: "no coverage" and "not measurable"
    /// are different facts and collapsing them is the dishonesty this field
    /// exists to prevent.
    pub coverage_pct: Option<f64>,
    /// Why the measurement could not be taken, when it could not.
    pub unmeasurable_reason: Option<String>,
}

/// Storage for the structural git-history index.
///
/// `Send + Sync` because the hybrid ranker holds it behind an `Arc<dyn …>` and
/// that ranker is used from the hook scorer, which must be `Send`.
pub trait HistoryStore: Send + Sync {
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

    /// Ranked commits for a query (spec §6.1/§6.2).
    fn search_commits(&self, query: &HistoryQuery) -> Result<Vec<HistoryCommitHit>>;

    /// The structural diff rows for one commit.
    fn commit_files(&self, sha: &str) -> Result<Vec<HistoryCommitFile>>;

    /// One commit by SHA, with its recency decay and file rows already
    /// attached. Exists so a caller that has a ranked list of SHAs hydrates
    /// through the *same* row-reading path as a search, rather than a second
    /// one that can disagree about how a commit is shaped. `score` carries the
    /// recency decay, there being no query to be relevant to.
    fn commit_hit_by_sha(&self, sha: &str) -> Result<Option<HistoryCommitHit>>;

    /// Files that most often change in the same commit as `file_path`
    /// (spec §6.4 Q7). Pure SQL over the structural index; no embeddings.
    fn co_changed_files(
        &self,
        repository: &str,
        file_path: &str,
        limit: usize,
    ) -> Result<Vec<CoChangedFile>>;

    /// Measured provenance reach for the repository (spec §10.1).
    fn provenance_coverage(&self, repository: &str) -> Result<ProvenanceCoverage>;
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

    /// Columns of `history_commits`, named explicitly so a `SELECT c.*` in a
    /// join cannot shift positions under the reader.
    const COMMIT_COLUMNS: &'static str = "c.sha, c.short_sha, c.parent_shas, c.is_merge, \
         c.author_name, c.author_email, c.authored_at, c.committed_at, c.subject, c.body, \
         c.branch_hint, c.repository";

    fn commit_from_row(row: &rusqlite::Row) -> rusqlite::Result<HistoryCommit> {
        let parents: String = row.get("parent_shas")?;
        Ok(HistoryCommit {
            sha: row.get("sha")?,
            short_sha: row.get("short_sha")?,
            parent_shas: serde_json::from_str(&parents).unwrap_or_default(),
            is_merge: row.get::<_, i64>("is_merge")? != 0,
            author_name: row.get("author_name")?,
            author_email: row.get("author_email")?,
            authored_at: row.get("authored_at")?,
            committed_at: row.get("committed_at")?,
            subject: row.get("subject")?,
            body: row.get("body")?,
            branch_hint: row.get("branch_hint")?,
            repository: row.get("repository")?,
        })
    }

    fn file_from_row(row: &rusqlite::Row) -> rusqlite::Result<HistoryCommitFile> {
        Ok(HistoryCommitFile {
            sha: row.get("sha")?,
            file_path: row.get("file_path")?,
            change_type: row.get("change_type")?,
            old_path: row.get("old_path")?,
            insertions: row.get("insertions")?,
            deletions: row.get("deletions")?,
        })
    }

    /// `0.5^(days/30)` — the same decay the temporal channel applies to entries
    /// (spec §6.2), so a commit and a memory of the same age agree on how old
    /// they are. Unparseable timestamps yield 0.0 rather than 1.0: an unknown
    /// age must not masquerade as "brand new".
    fn recency_decay(committed_at: &str) -> f64 {
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(committed_at) else {
            return 0.0;
        };
        let days = (chrono::Utc::now() - when.with_timezone(&chrono::Utc)).num_days() as f64;
        0.5f64.powf(days.max(0.0) / 30.0)
    }

    /// Build the shared `WHERE` tail for a history query.
    ///
    /// Returned as SQL text plus positional params so both the FTS branch and
    /// the structural branch apply *identical* filtering — the alternative is
    /// two filter implementations that drift, and a `path=` filter that means
    /// one thing with a query string and another without it.
    fn filter_sql(query: &HistoryQuery, next_param: usize) -> (String, Vec<String>) {
        let mut sql = String::new();
        let mut params: Vec<String> = Vec::new();
        let mut idx = next_param;

        if !query.include_merges {
            sql.push_str(" AND c.is_merge = 0");
        }
        if let Some(since) = &query.since {
            sql.push_str(&format!(" AND c.committed_at >= ?{idx}"));
            params.push(since.clone());
            idx += 1;
        }
        if let Some(until) = &query.until {
            sql.push_str(&format!(" AND c.committed_at <= ?{idx}"));
            params.push(until.clone());
            idx += 1;
        }
        if let Some(path) = &query.path {
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM history_commit_files f \
                   WHERE f.sha = c.sha AND (f.file_path LIKE '%' || ?{idx} || '%' \
                   OR f.old_path LIKE '%' || ?{idx} || '%'))"
            ));
            params.push(path.clone());
        }
        (sql, params)
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

        // Which of these commits are re-indexes? Only those need their FTS row
        // retired first. On a fresh backfill this set is empty, so the common
        // path issues no deletes at all — which matters because `sha` is an
        // UNINDEXED FTS column and a delete by it scans the whole index.
        let reindexed: Vec<&str> = {
            let mut stmt =
                tx.prepare("SELECT EXISTS(SELECT 1 FROM history_commits WHERE sha = ?1)")?;
            let mut seen = Vec::new();
            for c in commits {
                let exists: bool = stmt.query_row(params![c.sha], |row| row.get(0))?;
                if exists {
                    seen.push(c.sha.as_str());
                }
            }
            seen
        };

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

        // The FTS index is written in the SAME transaction as the rows it
        // describes, for the same reason the watermark is: a lexical index that
        // can disagree with the table it indexes is a silent-wrong-answer
        // machine, and nothing would ever reconcile the two.
        {
            let mut delete = tx.prepare("DELETE FROM history_commits_fts WHERE sha = ?1")?;
            for sha in &reindexed {
                delete.execute(params![sha])?;
            }
            let mut insert = tx.prepare(
                "INSERT INTO history_commits_fts (sha, subject, body) VALUES (?1, ?2, ?3)",
            )?;
            for c in commits {
                insert.execute(params![c.sha, c.subject, c.body.as_deref().unwrap_or("")])?;
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

    fn search_commits(&self, query: &HistoryQuery) -> Result<Vec<HistoryCommitHit>> {
        use rusqlite::types::Value;

        let limit = query.limit.max(1);
        let asked_for_text = query.text.as_ref().is_some_and(|t| !t.trim().is_empty());
        let text_expr = query
            .text
            .as_deref()
            .and_then(crate::fts_query::fts_or_query);

        // A text query that tokenizes to nothing (punctuation only) returns
        // nothing. Falling through to the structural branch would answer
        // "?????" with the whole repository, which reads as a match.
        if asked_for_text && text_expr.is_none() {
            return Ok(Vec::new());
        }

        let (sql, values) = match &text_expr {
            Some(expr) => {
                let (filter, filter_params) = Self::filter_sql(query, 3);
                let limit_idx = 3 + filter_params.len();
                let mut values = vec![
                    Value::Text(expr.clone()),
                    Value::Text(query.repository.clone()),
                ];
                values.extend(filter_params.into_iter().map(Value::Text));
                values.push(Value::Integer(limit as i64));
                (
                    format!(
                        "SELECT {cols}, bm25(history_commits_fts) AS score
                           FROM history_commits_fts
                           JOIN history_commits c ON c.sha = history_commits_fts.sha
                          WHERE history_commits_fts MATCH ?1
                            AND c.repository = ?2{filter}
                          ORDER BY score
                          LIMIT ?{limit_idx}",
                        cols = Self::COMMIT_COLUMNS,
                    ),
                    values,
                )
            }
            None => {
                let (filter, filter_params) = Self::filter_sql(query, 2);
                let limit_idx = 2 + filter_params.len();
                let mut values = vec![Value::Text(query.repository.clone())];
                values.extend(filter_params.into_iter().map(Value::Text));
                values.push(Value::Integer(limit as i64));
                (
                    format!(
                        "SELECT {cols}, 0.0 AS score
                           FROM history_commits c
                          WHERE c.repository = ?1{filter}
                          ORDER BY c.committed_at DESC, c.sha
                          LIMIT ?{limit_idx}",
                        cols = Self::COMMIT_COLUMNS,
                    ),
                    values,
                )
            }
        };

        let conn = self.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values), |row| {
                let raw: f64 = row.get("score")?;
                Ok((Self::commit_from_row(row)?, raw))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Files are fetched per hit rather than joined, so a commit touching 200
        // files cannot multiply the ranked row set out from under `LIMIT`.
        let mut files_stmt = match &query.path {
            Some(_) => conn.prepare(
                "SELECT * FROM history_commit_files
                  WHERE sha = ?1
                    AND (file_path LIKE '%' || ?2 || '%' OR old_path LIKE '%' || ?2 || '%')
                  ORDER BY file_path",
            )?,
            None => conn.prepare(
                "SELECT * FROM history_commit_files WHERE sha = ?1 ORDER BY file_path",
            )?,
        };

        let mut hits = Vec::with_capacity(rows.len());
        for (commit, raw_score) in rows {
            let files = match &query.path {
                Some(path) => files_stmt
                    .query_map(params![commit.sha, path], Self::file_from_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                None => files_stmt
                    .query_map(params![commit.sha], Self::file_from_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            };
            let recency = Self::recency_decay(&commit.committed_at);
            // SQLite's bm25() is a *cost*: 0 means "no information", negative
            // is a match and more negative is better. Negate so bigger = better,
            // matching every other channel's convention. Without a text query
            // there is no lexical signal at all, so recency is the score.
            let score = if text_expr.is_some() {
                (-raw_score).max(0.0)
            } else {
                recency
            };
            hits.push(HistoryCommitHit {
                commit,
                score,
                recency,
                files,
            });
        }
        Ok(hits)
    }

    fn commit_files(&self, sha: &str) -> Result<Vec<HistoryCommitFile>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM history_commit_files WHERE sha = ?1 ORDER BY file_path")?;
        let files = stmt
            .query_map(params![sha], Self::file_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    fn commit_hit_by_sha(&self, sha: &str) -> Result<Option<HistoryCommitHit>> {
        let commit = {
            let conn = self.lock();
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM history_commits c WHERE c.sha = ?1",
                Self::COMMIT_COLUMNS
            ))?;
            stmt.query_row(params![sha], Self::commit_from_row)
                .optional()?
        };
        let Some(commit) = commit else {
            return Ok(None);
        };
        let recency = Self::recency_decay(&commit.committed_at);
        Ok(Some(HistoryCommitHit {
            files: self.commit_files(sha)?,
            commit,
            score: recency,
            recency,
        }))
    }

    fn co_changed_files(
        &self,
        repository: &str,
        file_path: &str,
        limit: usize,
    ) -> Result<Vec<CoChangedFile>> {
        let conn = self.lock();
        // Merges are excluded: a merge commit "touches" every file both sides
        // changed, so counting them would report co-change between files that
        // were never edited together in any authored commit.
        let mut stmt = conn.prepare(
            "SELECT other.file_path AS file_path, COUNT(*) AS n
               FROM history_commit_files seed
               JOIN history_commit_files other
                 ON other.sha = seed.sha AND other.file_path <> seed.file_path
               JOIN history_commits c ON c.sha = seed.sha
              WHERE c.repository = ?1
                AND c.is_merge = 0
                AND seed.file_path = ?2
              GROUP BY other.file_path
              ORDER BY n DESC, other.file_path
              LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![repository, file_path, limit as i64], |row| {
                Ok(CoChangedFile {
                    file_path: row.get("file_path")?,
                    commits_together: row.get("n")?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn provenance_coverage(&self, repository: &str) -> Result<ProvenanceCoverage> {
        let conn = self.lock();
        let total_commits: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_commits WHERE repository = ?1",
            params![repository],
            |row| row.get(0),
        )?;

        // The edge lives in another subsystem's table. A store that has never
        // seen a task is a legitimate state (a fresh project, or the history
        // tables opened standalone in a test), so its absence is reported as
        // "not measurable" rather than as 0% — which would read as a finding.
        let has_tasks: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tasks')",
            [],
            |row| row.get(0),
        )?;
        if !has_tasks {
            return Ok(ProvenanceCoverage {
                total_commits,
                high_confidence_linked: None,
                coverage_pct: None,
                unmeasurable_reason: Some(
                    "no tasks table in this store: the factory_branch_anchor edge cannot be read"
                        .to_string(),
                ),
            });
        }

        let linked: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_commits c
              WHERE c.repository = ?1
                AND EXISTS (
                    SELECT 1 FROM tasks t
                     WHERE json_extract(t.deliverables, '$.factory_branch_anchor') = c.sha
                )",
            params![repository],
            |row| row.get(0),
        )?;

        Ok(ProvenanceCoverage {
            total_commits,
            high_confidence_linked: Some(linked),
            coverage_pct: (total_commits > 0)
                .then(|| linked as f64 * 100.0 / total_commits as f64),
            unmeasurable_reason: None,
        })
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

    // ── Query surface (EPIC cas-6212 / cas-7f40, M4) ────────────────────

    /// A commit with an explicit timestamp, so recency assertions are not at
    /// the mercy of when the suite runs.
    fn commit_at(sha: &str, subject: &str, body: &str, committed_at: &str) -> HistoryCommit {
        HistoryCommit {
            subject: subject.to_string(),
            body: Some(body.to_string()),
            committed_at: committed_at.to_string(),
            ..commit(sha)
        }
    }

    fn query(repository: &str) -> HistoryQuery {
        HistoryQuery {
            repository: repository.to_string(),
            limit: 10,
            ..Default::default()
        }
    }

    #[test]
    fn text_search_matches_subject_and_body() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[
                    commit_at(&a, "fix the redelivery hot loop", "", "2026-08-01T00:00:00Z"),
                    commit_at(&b, "unrelated", "stop re-emitting per poll tick", "2026-08-01T00:00:00Z"),
                ],
                &[],
                &b,
                true,
            )
            .unwrap();

        let hits = store
            .search_commits(&HistoryQuery {
                text: Some("redelivery".into()),
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].commit.sha, a);
        assert!(hits[0].score > 0.0, "bm25() cost must be negated into a relevance");

        // Body text is indexed too — the whole point of embedding prose rather
        // than subjects alone.
        let hits = store
            .search_commits(&HistoryQuery {
                text: Some("emitting".into()),
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].commit.sha, b);
    }

    /// cas-461a in this store's dialect: multi-term queries must OR, or recall
    /// collapses as the question gets more specific.
    #[test]
    fn multi_term_queries_do_not_require_every_term() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[commit_at(&a, "fix the verifier gate", "", "2026-08-01T00:00:00Z")],
                &[],
                &a,
                true,
            )
            .unwrap();

        let hits = store
            .search_commits(&HistoryQuery {
                text: Some("verifier scheduler embeddings telemetry".into()),
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1, "AND semantics would return nothing here");
    }

    /// Punctuation-only input must not degrade into "return everything".
    #[test]
    fn an_untokenizable_query_returns_nothing_not_everything() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();

        let hits = store
            .search_commits(&HistoryQuery {
                text: Some("??? ---".into()),
                ..query("/repo")
            })
            .unwrap();
        assert!(hits.is_empty());
    }

    /// Q2: "what changed in <path> in the last two weeks" — no text at all.
    #[test]
    fn structural_query_filters_by_path_and_window() {
        let (_t, store) = store();
        let old = "a".repeat(40);
        let new = "b".repeat(40);
        let other = "c".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[
                    commit_at(&old, "old delivery change", "", "2026-01-01T00:00:00Z"),
                    commit_at(&new, "new delivery change", "", "2026-08-05T00:00:00Z"),
                    commit_at(&other, "unrelated area", "", "2026-08-05T00:00:00Z"),
                ],
                &[
                    file_row(&old, "src/delivery/state.rs"),
                    file_row(&new, "src/delivery/state.rs"),
                    file_row(&other, "src/ui/pane.rs"),
                ],
                &other,
                true,
            )
            .unwrap();

        let hits = store
            .search_commits(&HistoryQuery {
                path: Some("src/delivery".into()),
                since: Some("2026-08-01T00:00:00Z".into()),
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].commit.sha, new);
        assert_eq!(hits[0].files.len(), 1, "files are narrowed to the path filter");
        assert_eq!(hits[0].files[0].file_path, "src/delivery/state.rs");

        // `until` bounds the other side of the window.
        let hits = store
            .search_commits(&HistoryQuery {
                path: Some("src/delivery".into()),
                until: Some("2026-02-01T00:00:00Z".into()),
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].commit.sha, old);
    }

    /// The path filter must apply identically with and without a text query —
    /// the failure mode of maintaining two filter implementations.
    #[test]
    fn path_filter_applies_in_the_text_branch_too() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[
                    commit_at(&a, "retry backoff", "", "2026-08-05T00:00:00Z"),
                    commit_at(&b, "retry backoff", "", "2026-08-05T00:00:00Z"),
                ],
                &[file_row(&a, "src/delivery/retry.rs"), file_row(&b, "docs/notes.md")],
                &b,
                true,
            )
            .unwrap();

        let hits = store
            .search_commits(&HistoryQuery {
                text: Some("retry backoff".into()),
                path: Some("src/delivery".into()),
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].commit.sha, a);
    }

    /// Merge commits carry `Merge branch 'x'` as their whole message, so they
    /// are noise by default — but a structural question can ask for them.
    #[test]
    fn merges_are_excluded_by_default_and_includable_on_request() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let mut merge = commit_at(&a, "Merge branch 'x'", "", "2026-08-05T00:00:00Z");
        merge.is_merge = true;
        merge.parent_shas = vec!["p".repeat(40), "q".repeat(40)];
        store
            .commit_batch("/repo", &[merge], &[], &a, true)
            .unwrap();

        assert!(store.search_commits(&query("/repo")).unwrap().is_empty());
        let hits = store
            .search_commits(&HistoryQuery {
                include_merges: true,
                ..query("/repo")
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    /// Re-indexing a commit must replace its FTS row, not add a second one —
    /// a duplicated row would double its term frequencies and skew bm25().
    #[test]
    fn reindexing_replaces_the_fts_row_rather_than_duplicating_it() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        for subject in ["original subject", "amended subject"] {
            store
                .commit_batch(
                    "/repo",
                    &[commit_at(&a, subject, "", "2026-08-05T00:00:00Z")],
                    &[],
                    &a,
                    true,
                )
                .unwrap();
        }

        let rows: i64 = {
            let conn = store.lock();
            conn.query_row("SELECT COUNT(*) FROM history_commits_fts", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(rows, 1, "stale FTS row survived a re-index");

        assert!(
            store
                .search_commits(&HistoryQuery {
                    text: Some("original".into()),
                    ..query("/repo")
                })
                .unwrap()
                .is_empty(),
            "the superseded subject is still searchable"
        );
        assert_eq!(
            store
                .search_commits(&HistoryQuery {
                    text: Some("amended".into()),
                    ..query("/repo")
                })
                .unwrap()
                .len(),
            1
        );
    }

    /// Q7: co-change over the structural index, no embeddings involved.
    #[test]
    fn co_changed_files_ranks_by_shared_commits() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let c = "c".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[commit(&a), commit(&b), commit(&c)],
                &[
                    file_row(&a, "scorer.rs"),
                    file_row(&a, "hybrid.rs"),
                    file_row(&b, "scorer.rs"),
                    file_row(&b, "hybrid.rs"),
                    file_row(&c, "scorer.rs"),
                    file_row(&c, "docs.md"),
                ],
                &c,
                true,
            )
            .unwrap();

        let co = store.co_changed_files("/repo", "scorer.rs", 10).unwrap();
        assert_eq!(co[0].file_path, "hybrid.rs");
        assert_eq!(co[0].commits_together, 2);
        assert_eq!(co[1].file_path, "docs.md");
        assert_eq!(co[1].commits_together, 1);
        assert!(
            !co.iter().any(|r| r.file_path == "scorer.rs"),
            "a file must not co-change with itself"
        );
    }

    /// A merge "touches" everything both sides changed, so counting merges
    /// would invent co-change between files nobody ever edited together.
    #[test]
    fn co_change_ignores_merge_commits() {
        let (_t, store) = store();
        let m = "a".repeat(40);
        let mut merge = commit(&m);
        merge.is_merge = true;
        store
            .commit_batch(
                "/repo",
                &[merge],
                &[file_row(&m, "scorer.rs"), file_row(&m, "unrelated.rs")],
                &m,
                true,
            )
            .unwrap();

        assert!(store.co_changed_files("/repo", "scorer.rs", 10).unwrap().is_empty());
    }

    /// With no tasks table the coverage is *unmeasurable*, which is a different
    /// fact from 0% and must not be rendered as one (spec §10.1).
    #[test]
    fn provenance_coverage_reports_unmeasurable_rather_than_zero() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();

        let cov = store.provenance_coverage("/repo").unwrap();
        assert_eq!(cov.total_commits, 1);
        assert!(cov.high_confidence_linked.is_none());
        assert!(cov.coverage_pct.is_none());
        assert!(cov.unmeasurable_reason.is_some());
    }

    #[test]
    fn provenance_coverage_counts_the_factory_branch_anchor_edge() {
        let (_t, store) = store();
        let linked = "a".repeat(40);
        let unlinked = "b".repeat(40);
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE tasks (id TEXT PRIMARY KEY, deliverables TEXT NOT NULL DEFAULT '{}');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, deliverables) VALUES ('cas-1', json_object('factory_branch_anchor', ?1))",
                params![linked],
            )
            .unwrap();
        }
        store
            .commit_batch(
                "/repo",
                &[commit(&linked), commit(&unlinked)],
                &[],
                &unlinked,
                true,
            )
            .unwrap();

        let cov = store.provenance_coverage("/repo").unwrap();
        assert_eq!(cov.total_commits, 2);
        assert_eq!(cov.high_confidence_linked, Some(1));
        assert_eq!(cov.coverage_pct, Some(50.0));
        assert!(cov.unmeasurable_reason.is_none());
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
