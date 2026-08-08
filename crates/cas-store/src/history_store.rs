//! Structural git-history index (EPIC cas-6212 / cas-7a21, spec §4).
//!
//! Three tables, all project-scoped:
//!
//! - `history_commits` — one row per commit (subject/body/author/timestamps).
//! - `history_commit_files` — the structural diff mapping, one row per
//!   `(commit, file)` pair. Diffs are indexed *structurally*: which files a
//!   commit touched and how much, never the hunk text (spec §3, which makes
//!   this a privacy property as well as a cost one).
//! - `history_docs` — GitHub issues/PRs/comments and CHANGELOG release
//!   sections, one row per embeddable text unit (spec §4.1, §8; cas-9a38).
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

/// Canonical DDL for `history_docs` (spec §4.1, §8 — EPIC cas-6212 / cas-9a38).
///
/// Kept as its own constant rather than folded into [`HISTORY_SCHEMA`] so the
/// M1 migration (`m221`) and the M6 migration (`m222`) each own exactly the
/// tables they introduced; a database that stopped at m221 must not suddenly
/// grow an M6 table because a shared constant moved under it.
///
/// # Two columns the spec's sketch does not list, and why
///
/// `repository` and `source` are added deliberately. `history_index_state` is
/// keyed `(repository, source)`, so every doc must be attributable to the same
/// pair or the ledger describes rows it cannot name — "GitHub is 400 docs
/// behind" is unanswerable when the docs themselves do not record which source
/// produced them. They also keep a multi-repo store from blending two projects'
/// issues into one corpus, which is the same reason `history_commits` carries
/// `repository`.
pub const HISTORY_DOCS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_docs (
    id TEXT PRIMARY KEY,
    doc_kind TEXT NOT NULL,
    number INTEGER,
    title TEXT,
    body TEXT,
    state TEXT,
    author TEXT,
    created_at TEXT,
    updated_at TEXT,
    closed_at TEXT,
    url TEXT,
    refs_json TEXT,
    repository TEXT NOT NULL,
    source TEXT NOT NULL,
    pending_embedding INTEGER NOT NULL DEFAULT 1,
    fetched_at TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'project'
);

CREATE INDEX IF NOT EXISTS idx_history_docs_kind
    ON history_docs(doc_kind);
CREATE INDEX IF NOT EXISTS idx_history_docs_updated_at
    ON history_docs(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_history_docs_repo_source
    ON history_docs(repository, source);
CREATE INDEX IF NOT EXISTS idx_history_docs_pending_embedding
    ON history_docs(updated_at) WHERE pending_embedding = 1;
"#;

/// Statement-level form of [`HISTORY_DOCS_SCHEMA`] for the migration runner.
/// Keep in lockstep; `m222`'s shape-drift test fails on any divergence.
pub const HISTORY_DOCS_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS history_docs (
        id TEXT PRIMARY KEY,
        doc_kind TEXT NOT NULL,
        number INTEGER,
        title TEXT,
        body TEXT,
        state TEXT,
        author TEXT,
        created_at TEXT,
        updated_at TEXT,
        closed_at TEXT,
        url TEXT,
        refs_json TEXT,
        repository TEXT NOT NULL,
        source TEXT NOT NULL,
        pending_embedding INTEGER NOT NULL DEFAULT 1,
        fetched_at TEXT NOT NULL,
        scope TEXT NOT NULL DEFAULT 'project'
    )",
    "CREATE INDEX IF NOT EXISTS idx_history_docs_kind
        ON history_docs(doc_kind)",
    "CREATE INDEX IF NOT EXISTS idx_history_docs_updated_at
        ON history_docs(updated_at DESC)",
    "CREATE INDEX IF NOT EXISTS idx_history_docs_repo_source
        ON history_docs(repository, source)",
    "CREATE INDEX IF NOT EXISTS idx_history_docs_pending_embedding
        ON history_docs(updated_at) WHERE pending_embedding = 1",
];

/// The `history_index_state.source` value for the git walker.
pub const SOURCE_GIT: &str = "git";

/// The `history_index_state.source` value for the GitHub issue/PR indexer
/// (spec §8).
pub const SOURCE_GITHUB: &str = "github";

/// The `history_index_state.source` value for the CHANGELOG release parser.
pub const SOURCE_CHANGELOG: &str = "changelog";

/// `history_docs.doc_kind` values. These are the id prefixes too
/// (`gh:issue:116`, `gh:pr:57`, `gh:comment:<id>`, `changelog:v2.49.0`).
pub const DOC_KIND_ISSUE: &str = "issue";
pub const DOC_KIND_PR: &str = "pr";
pub const DOC_KIND_COMMENT: &str = "comment";
pub const DOC_KIND_CHANGELOG: &str = "changelog";

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

/// One embeddable text unit from GitHub or the CHANGELOG (spec §4.1, §8).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryDoc {
    /// Namespaced id: `gh:issue:116`, `gh:pr:57`, `gh:comment:<node-id>`,
    /// `changelog:v2.49.0`.
    pub id: String,
    /// One of [`DOC_KIND_ISSUE`], [`DOC_KIND_PR`], [`DOC_KIND_COMMENT`],
    /// [`DOC_KIND_CHANGELOG`].
    pub doc_kind: String,
    /// Issue/PR number; the issue number for a comment; `None` for CHANGELOG.
    pub number: Option<i64>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    pub url: Option<String>,
    /// JSON object of extracted references — commit SHAs, issue numbers, task
    /// ids, and for a merged PR its merge-commit SHA (spec §8, PR↔commit).
    pub refs_json: Option<String>,
    pub repository: String,
    pub source: String,
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

    /// Write a batch of docs and the `(repository, source)` watermark in ONE
    /// transaction — the same rule `commit_batch` enforces for git (spec §4.2
    /// rule 2). A half-written GitHub page must re-fetch, never be skipped.
    ///
    /// `watermark_at` is the **data** cursor for this source: the newest
    /// `updated_at` the pass observed, not the wall clock. Taking it from the
    /// data means a clock difference between this machine and GitHub can only
    /// cause a harmless re-fetch of the boundary item, never a skipped one.
    /// Pass `None` to leave an existing cursor alone (a pass that found nothing
    /// new must not advance it).
    ///
    /// Returns the number of doc rows written.
    fn upsert_docs(
        &self,
        repository: &str,
        source: &str,
        docs: &[HistoryDoc],
        watermark_at: Option<&str>,
        backfill_complete: bool,
    ) -> Result<usize>;

    /// `(kind, count)` pairs for a repository, kind-ascending.
    fn doc_counts(&self, repository: &str) -> Result<Vec<(String, i64)>>;

    /// How many docs still await an embedding (M7's queue depth, reported by
    /// `cas history status` so the backlog is visible before M7 exists).
    fn docs_pending_embedding(&self, repository: &str) -> Result<i64>;

    /// Read one doc by id.
    fn get_doc(&self, id: &str) -> Result<Option<HistoryDoc>>;
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

    fn doc_from_row(row: &rusqlite::Row) -> rusqlite::Result<HistoryDoc> {
        Ok(HistoryDoc {
            id: row.get("id")?,
            doc_kind: row.get("doc_kind")?,
            number: row.get("number")?,
            title: row.get("title")?,
            body: row.get("body")?,
            state: row.get("state")?,
            author: row.get("author")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            closed_at: row.get("closed_at")?,
            url: row.get("url")?,
            refs_json: row.get("refs_json")?,
            repository: row.get("repository")?,
            source: row.get("source")?,
        })
    }
}

impl HistoryStore for SqliteHistoryStore {
    fn init(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(HISTORY_SCHEMA)?;
        conn.execute_batch(HISTORY_DOCS_SCHEMA)?;
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

    fn upsert_docs(
        &self,
        repository: &str,
        source: &str,
        docs: &[HistoryDoc],
        watermark_at: Option<&str>,
        backfill_complete: bool,
    ) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        {
            // `pending_embedding` is re-armed only when the EMBEDDED text
            // changes. Spec §4.4 embeds `title + body`; a state flip from OPEN
            // to CLOSED, or a fresh `updated_at`, changes neither. Re-arming on
            // every touch would make a `--force` re-fetch enqueue the whole
            // corpus for re-embedding — 116 issues + 198 comments of paid,
            // rate-limited work for identical vectors.
            let mut stmt = tx.prepare(
                "INSERT INTO history_docs (
                     id, doc_kind, number, title, body, state, author,
                     created_at, updated_at, closed_at, url, refs_json,
                     repository, source, pending_embedding, fetched_at, scope
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           ?13, ?14, 1, ?15, 'project')
                 ON CONFLICT(id) DO UPDATE SET
                     doc_kind = excluded.doc_kind,
                     number = excluded.number,
                     title = excluded.title,
                     body = excluded.body,
                     state = excluded.state,
                     author = excluded.author,
                     created_at = excluded.created_at,
                     updated_at = excluded.updated_at,
                     closed_at = excluded.closed_at,
                     url = excluded.url,
                     refs_json = excluded.refs_json,
                     repository = excluded.repository,
                     source = excluded.source,
                     fetched_at = excluded.fetched_at,
                     pending_embedding = CASE
                         WHEN history_docs.title IS NOT excluded.title
                           OR history_docs.body IS NOT excluded.body
                         THEN 1
                         ELSE history_docs.pending_embedding
                     END",
            )?;
            for d in docs {
                stmt.execute(params![
                    d.id,
                    d.doc_kind,
                    d.number,
                    d.title,
                    d.body,
                    d.state,
                    d.author,
                    d.created_at,
                    d.updated_at,
                    d.closed_at,
                    d.url,
                    d.refs_json,
                    repository,
                    source,
                    now,
                ])?;
            }
        }

        // The cursor never moves backwards, and a pass that found nothing new
        // (`watermark_at = None`) leaves it exactly where it was — an empty
        // fetch is evidence of nothing to do, not evidence of freshness at
        // wall-clock time.
        tx.execute(
            "INSERT INTO history_index_state (
                 repository, source, last_indexed_at, last_attempt_at,
                 last_error, backfill_complete, items_indexed
             ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)
             ON CONFLICT(repository, source) DO UPDATE SET
                 last_indexed_at = NULLIF(
                     MAX(
                         COALESCE(excluded.last_indexed_at, ''),
                         COALESCE(history_index_state.last_indexed_at, '')
                     ), ''),
                 last_attempt_at = excluded.last_attempt_at,
                 last_error = NULL,
                 backfill_complete = excluded.backfill_complete,
                 items_indexed = history_index_state.items_indexed
                                 + excluded.items_indexed",
            params![
                repository,
                source,
                watermark_at,
                now,
                i64::from(backfill_complete),
                docs.len() as i64,
            ],
        )?;

        tx.commit()?;
        Ok(docs.len())
    }

    fn doc_counts(&self, repository: &str) -> Result<Vec<(String, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT doc_kind, COUNT(*) FROM history_docs
              WHERE repository = ?1 GROUP BY doc_kind ORDER BY doc_kind",
        )?;
        let rows = stmt
            .query_map(params![repository], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn docs_pending_embedding(&self, repository: &str) -> Result<i64> {
        let conn = self.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_docs
              WHERE repository = ?1 AND pending_embedding = 1",
            params![repository],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    fn get_doc(&self, id: &str) -> Result<Option<HistoryDoc>> {
        let conn = self.lock();
        let doc = conn
            .query_row(
                "SELECT * FROM history_docs WHERE id = ?1",
                params![id],
                Self::doc_from_row,
            )
            .optional()?;
        Ok(doc)
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

    fn doc(id: &str, kind: &str, title: &str, body: &str, updated: &str) -> HistoryDoc {
        HistoryDoc {
            id: id.to_string(),
            doc_kind: kind.to_string(),
            number: Some(1),
            title: Some(title.to_string()),
            body: Some(body.to_string()),
            state: Some("OPEN".into()),
            author: Some("ada".into()),
            created_at: Some("2026-08-01T00:00:00Z".into()),
            updated_at: Some(updated.to_string()),
            closed_at: None,
            url: Some(format!("https://example.test/{id}")),
            refs_json: Some("{}".into()),
            repository: "/repo".into(),
            source: SOURCE_GITHUB.into(),
        }
    }

    #[test]
    fn upsert_docs_writes_rows_and_advances_the_data_cursor() {
        let (_t, store) = store();
        let d = doc("gh:issue:1", DOC_KIND_ISSUE, "t", "b", "2026-08-02T00:00:00Z");
        assert_eq!(
            store
                .upsert_docs(
                    "/repo",
                    SOURCE_GITHUB,
                    std::slice::from_ref(&d),
                    Some("2026-08-02T00:00:00Z"),
                    true,
                )
                .unwrap(),
            1
        );

        let state = store.index_state("/repo", SOURCE_GITHUB).unwrap().unwrap();
        assert_eq!(
            state.last_indexed_at.as_deref(),
            Some("2026-08-02T00:00:00Z"),
            "cursor must be the data timestamp, not the wall clock"
        );
        assert!(state.backfill_complete);
        assert_eq!(state.items_indexed, 1);
        assert_eq!(store.get_doc("gh:issue:1").unwrap().unwrap(), d);
        // The git watermark is a different `source` row and must be untouched.
        assert!(store.index_state("/repo", SOURCE_GIT).unwrap().is_none());
    }

    /// A pass that fetched nothing must not restamp the cursor: an empty result
    /// is "nothing to do", never "fresh as of now" (spec §10.1).
    #[test]
    fn an_empty_pass_leaves_the_cursor_where_it_was() {
        let (_t, store) = store();
        store
            .upsert_docs(
                "/repo",
                SOURCE_GITHUB,
                &[doc("gh:issue:1", DOC_KIND_ISSUE, "t", "b", "2026-08-02T00:00:00Z")],
                Some("2026-08-02T00:00:00Z"),
                true,
            )
            .unwrap();
        store
            .upsert_docs("/repo", SOURCE_GITHUB, &[], None, true)
            .unwrap();

        let state = store.index_state("/repo", SOURCE_GITHUB).unwrap().unwrap();
        assert_eq!(
            state.last_indexed_at.as_deref(),
            Some("2026-08-02T00:00:00Z")
        );
        assert!(
            state.last_attempt_at.is_some(),
            "the attempt itself must still be recorded"
        );
    }

    #[test]
    fn the_doc_cursor_never_moves_backwards() {
        let (_t, store) = store();
        for at in ["2026-08-05T00:00:00Z", "2026-08-01T00:00:00Z"] {
            store
                .upsert_docs("/repo", SOURCE_GITHUB, &[], Some(at), true)
                .unwrap();
        }
        let state = store.index_state("/repo", SOURCE_GITHUB).unwrap().unwrap();
        assert_eq!(
            state.last_indexed_at.as_deref(),
            Some("2026-08-05T00:00:00Z")
        );
    }

    /// Re-arming the embedding queue on every touch would make a `--force`
    /// re-fetch pay to re-embed an unchanged corpus.
    #[test]
    fn pending_embedding_rearms_on_text_change_only() {
        let (_t, store) = store();
        let base = doc("gh:issue:1", DOC_KIND_ISSUE, "t", "b", "2026-08-02T00:00:00Z");
        let mark_embedded = || {
            let conn = store.lock();
            conn.execute("UPDATE history_docs SET pending_embedding = 0", [])
                .unwrap();
        };

        store
            .upsert_docs("/repo", SOURCE_GITHUB, &[base.clone()], None, true)
            .unwrap();
        assert_eq!(store.docs_pending_embedding("/repo").unwrap(), 1);
        mark_embedded();

        // State + timestamps changed, embedded text did not.
        let mut restated = base.clone();
        restated.state = Some("CLOSED".into());
        restated.closed_at = Some("2026-08-09T00:00:00Z".into());
        restated.updated_at = Some("2026-08-09T00:00:00Z".into());
        store
            .upsert_docs("/repo", SOURCE_GITHUB, &[restated.clone()], None, true)
            .unwrap();
        assert_eq!(
            store.docs_pending_embedding("/repo").unwrap(),
            0,
            "a state flip must not re-enqueue an unchanged body"
        );
        assert_eq!(
            store.get_doc("gh:issue:1").unwrap().unwrap().state.as_deref(),
            Some("CLOSED"),
            "…but the row itself must still be updated"
        );

        let mut edited = restated;
        edited.body = Some("edited body".into());
        store
            .upsert_docs("/repo", SOURCE_GITHUB, &[edited], None, true)
            .unwrap();
        assert_eq!(store.docs_pending_embedding("/repo").unwrap(), 1);
    }

    #[test]
    fn doc_counts_are_per_kind_and_per_repository() {
        let (_t, store) = store();
        let mut elsewhere = doc("gh:issue:9", DOC_KIND_ISSUE, "t", "b", "2026-08-02T00:00:00Z");
        elsewhere.repository = "/elsewhere".into();
        store
            .upsert_docs(
                "/repo",
                SOURCE_GITHUB,
                &[
                    doc("gh:issue:1", DOC_KIND_ISSUE, "t", "b", "2026-08-02T00:00:00Z"),
                    doc("gh:pr:2", DOC_KIND_PR, "t", "b", "2026-08-02T00:00:00Z"),
                    doc("gh:comment:3", DOC_KIND_COMMENT, "t", "b", "2026-08-02T00:00:00Z"),
                ],
                None,
                true,
            )
            .unwrap();
        store
            .upsert_docs("/elsewhere", SOURCE_GITHUB, &[elsewhere], None, true)
            .unwrap();

        assert_eq!(
            store.doc_counts("/repo").unwrap(),
            vec![
                ("comment".to_string(), 1),
                ("issue".to_string(), 1),
                ("pr".to_string(), 1),
            ]
        );
        assert_eq!(
            store.doc_counts("/elsewhere").unwrap(),
            vec![("issue".to_string(), 1)]
        );
    }

    /// github and changelog keep independent ledgers; neither may clobber the
    /// other's cursor or the git walker's watermark.
    #[test]
    fn sources_keep_independent_ledgers() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();
        store
            .upsert_docs(
                "/repo",
                SOURCE_GITHUB,
                &[doc("gh:issue:1", DOC_KIND_ISSUE, "t", "b", "2026-08-02T00:00:00Z")],
                Some("2026-08-02T00:00:00Z"),
                true,
            )
            .unwrap();
        let mut cl = doc(
            "changelog:v2.49.0",
            DOC_KIND_CHANGELOG,
            "v2.49.0",
            "notes",
            "2026-08-07T00:00:00Z",
        );
        cl.source = SOURCE_CHANGELOG.into();
        cl.number = None;
        store
            .upsert_docs("/repo", SOURCE_CHANGELOG, &[cl], None, true)
            .unwrap();

        assert_eq!(
            store
                .index_state("/repo", SOURCE_GIT)
                .unwrap()
                .unwrap()
                .last_indexed_sha
                .as_deref(),
            Some(a.as_str())
        );
        assert_eq!(
            store
                .index_state("/repo", SOURCE_GITHUB)
                .unwrap()
                .unwrap()
                .items_indexed,
            1
        );
        assert_eq!(
            store
                .index_state("/repo", SOURCE_CHANGELOG)
                .unwrap()
                .unwrap()
                .items_indexed,
            1
        );
    }

    /// Offline: `record_attempt` must be able to file a github failure without
    /// disturbing anything the git half indexed (spec §10.2).
    #[test]
    fn a_github_failure_is_recorded_without_touching_the_git_watermark() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        store
            .commit_batch("/repo", &[commit(&a)], &[], &a, true)
            .unwrap();
        store
            .record_attempt("/repo", SOURCE_GITHUB, Some("gh not authenticated"))
            .unwrap();

        let github = store.index_state("/repo", SOURCE_GITHUB).unwrap().unwrap();
        assert_eq!(github.last_error.as_deref(), Some("gh not authenticated"));
        assert!(github.last_indexed_at.is_none());
        let git = store.index_state("/repo", SOURCE_GIT).unwrap().unwrap();
        assert_eq!(git.last_indexed_sha.as_deref(), Some(a.as_str()));
        assert!(git.last_error.is_none());
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
