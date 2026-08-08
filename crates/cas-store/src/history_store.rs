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

CREATE VIRTUAL TABLE IF NOT EXISTS history_commits_fts USING fts5(
    sha UNINDEXED,
    subject,
    body
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

/// Canonical DDL for `history_epochs` (spec §9 — EPIC cas-6212 / cas-8d2a, M8).
///
/// One row per observed *binary epoch*: a window during which a particular
/// executable was actually running. This is the table that makes "is symptom X
/// fixed" answerable against the running binary rather than a tag date, which
/// is the mistake cas-9d92 had to retract on 2026-08-07 (a fix installed at
/// 21:02:26Z while pre-install daemons kept heartbeating until 21:36:37Z).
///
/// `ended_at` is the *last observed liveness* of that process, not a clean
/// shutdown stamp — a killed daemon never gets to write one, so the heartbeat
/// advances it on every tick. A NULL `ended_at` therefore means "started and
/// never observed alive again", which is a weaker claim than "still running"
/// and is treated as such by the classifier.
pub const HISTORY_EPOCHS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_epochs (
    id INTEGER PRIMARY KEY,
    epoch_kind TEXT NOT NULL,
    binary_path TEXT,
    binary_mtime TEXT,
    version TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    pid INTEGER,
    exe_deleted INTEGER NOT NULL DEFAULT 0,
    recorded_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_history_epochs_started_at
    ON history_epochs(started_at);
CREATE INDEX IF NOT EXISTS idx_history_epochs_kind
    ON history_epochs(epoch_kind);
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_epochs_identity
    ON history_epochs(epoch_kind, COALESCE(pid, -1), started_at);
"#;

/// Statement-level form of [`HISTORY_EPOCHS_SCHEMA`] for the migration runner.
/// Keep in lockstep; `m226`'s shape-drift test fails on any divergence.
pub const HISTORY_EPOCHS_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS history_epochs (
        id INTEGER PRIMARY KEY,
        epoch_kind TEXT NOT NULL,
        binary_path TEXT,
        binary_mtime TEXT,
        version TEXT,
        started_at TEXT NOT NULL,
        ended_at TEXT,
        pid INTEGER,
        exe_deleted INTEGER NOT NULL DEFAULT 0,
        recorded_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_history_epochs_started_at
        ON history_epochs(started_at)",
    "CREATE INDEX IF NOT EXISTS idx_history_epochs_kind
        ON history_epochs(epoch_kind)",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_history_epochs_identity
        ON history_epochs(epoch_kind, COALESCE(pid, -1), started_at)",
];

/// `history_epochs.epoch_kind` values (spec §9).
pub const EPOCH_KIND_BINARY_INSTALL: &str = "binary_install";
pub const EPOCH_KIND_DAEMON_START: &str = "daemon_start";
pub const EPOCH_KIND_DAEMON_LAST_HEARTBEAT: &str = "daemon_last_heartbeat";

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

/// One observed binary epoch (spec §9).
///
/// `id` is ignored on write; the identity of a row is
/// `(epoch_kind, pid, started_at)`, which is what makes recording idempotent
/// across a daemon that re-registers and across repeated backfills.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEpoch {
    pub id: i64,
    /// One of [`EPOCH_KIND_BINARY_INSTALL`], [`EPOCH_KIND_DAEMON_START`],
    /// [`EPOCH_KIND_DAEMON_LAST_HEARTBEAT`].
    pub epoch_kind: String,
    pub binary_path: Option<String>,
    /// Executable mtime, RFC3339. This — not `version` — is what distinguishes
    /// two builds carrying the same `CARGO_PKG_VERSION`, which is the usual
    /// shape during a release day.
    pub binary_mtime: Option<String>,
    pub version: Option<String>,
    pub started_at: String,
    /// Last moment this process was *observed alive*. See
    /// [`HISTORY_EPOCHS_SCHEMA`] for why this is not a shutdown stamp.
    pub ended_at: Option<String>,
    pub pid: Option<i64>,
    /// `/proc/<pid>/exe` no longer resolves to the file on disk — the binary
    /// was replaced or removed under the running process.
    pub exe_deleted: bool,
    pub recorded_at: String,
}

/// What an epoch backfill pass actually found (spec §9 "historical rows are
/// backfilled from `daemon_instances`").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpochBackfill {
    /// False when `daemon_instances` does not exist in this database — a
    /// distinct fact from "no rows", and not reported as one.
    pub source_available: bool,
    pub scanned: i64,
    pub inserted: i64,
    /// Rows whose `(kind, pid, started_at)` identity was already present.
    pub already_present: i64,
}

/// Observation counts inside one time window, used by the Q6 verdict logic.
///
/// `sample` is the denominator the `INSUFFICIENT-POST-FIX-DATA` threshold is
/// tested against; `matches` is the symptom. Both are always carried so a
/// caller is never handed a bare "fixed" (spec §9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservationCounts {
    pub matches: i64,
    pub sample: i64,
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

    /// Record (or refresh) one binary epoch, keyed on
    /// `(epoch_kind, pid, started_at)`. Returns the row id.
    ///
    /// Idempotent by construction: a daemon that re-registers, or a backfill
    /// that runs twice, updates the existing row rather than growing a second
    /// window for the same process — duplicated epochs would widen the MIXED
    /// band and silently suppress CLEAN-POST evidence.
    fn record_epoch(&self, epoch: &HistoryEpoch) -> Result<i64>;

    /// Advance the `ended_at` of the newest `daemon_start` epoch for `pid`.
    ///
    /// Called from the daemon heartbeat rather than from shutdown: a killed or
    /// crashed daemon never reaches shutdown, and it is exactly those processes
    /// — the ones still serving an old binary — whose tail defines the MIXED
    /// window. Returns whether a row was updated.
    fn touch_epoch_end(&self, pid: i64, ended_at: &str) -> Result<bool>;

    /// Epochs ordered by `started_at` ascending, optionally from a lower bound.
    fn list_epochs(&self, since: Option<&str>, limit: usize) -> Result<Vec<HistoryEpoch>>;

    /// Backfill `daemon_start` epochs from `daemon_instances`.
    ///
    /// Those rows carry `started_at` and `last_heartbeat` but no binary
    /// identity, so the backfilled epochs have NULL `version`/`binary_mtime`.
    /// That is deliberate: an unknown binary cannot be claimed to be running a
    /// fix, and the classifier treats it as such (it can extend the MIXED
    /// window, never open a CLEAN-POST one).
    fn backfill_epochs_from_daemons(&self) -> Result<EpochBackfill>;

    /// Symptom and sample counts over `events` inside `[from, until)`.
    ///
    /// `symptom` is matched case-insensitively as a substring against
    /// `event_type` and `summary`. Comparison goes through SQLite's
    /// `datetime()` on both sides so a stored `…Z` and a stored `…+00:00`
    /// cannot order differently as strings.
    fn observation_counts(
        &self,
        from: &str,
        until: Option<&str>,
        symptom: Option<&str>,
    ) -> Result<ObservationCounts>;
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
        conn.execute_batch(HISTORY_EPOCHS_SCHEMA)?;
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

    fn record_epoch(&self, epoch: &HistoryEpoch) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        // Identity lookup rather than `ON CONFLICT`, because the uniqueness
        // constraint is an *expression* index (`COALESCE(pid, -1)`) and naming
        // an expression as a conflict target is a syntax that varies with the
        // SQLite build. A select-then-write inside the transaction is portable
        // and reads the same.
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM history_epochs
                  WHERE epoch_kind = ?1 AND COALESCE(pid, -1) = COALESCE(?2, -1)
                    AND started_at = ?3",
                params![epoch.epoch_kind, epoch.pid, epoch.started_at],
                |row| row.get(0),
            )
            .optional()?;

        let id = match existing {
            Some(id) => {
                // `ended_at` only ever moves forward: a refresh that carries no
                // liveness stamp must not erase one we already observed.
                tx.execute(
                    "UPDATE history_epochs
                        SET binary_path = COALESCE(?2, binary_path),
                            binary_mtime = COALESCE(?3, binary_mtime),
                            version = COALESCE(?4, version),
                            ended_at = MAX(COALESCE(?5, ''), COALESCE(ended_at, '')),
                            exe_deleted = ?6
                      WHERE id = ?1",
                    params![
                        id,
                        epoch.binary_path,
                        epoch.binary_mtime,
                        epoch.version,
                        epoch.ended_at,
                        i64::from(epoch.exe_deleted),
                    ],
                )?;
                // MAX() of two empty strings writes '' where NULL is meant.
                tx.execute(
                    "UPDATE history_epochs SET ended_at = NULL WHERE id = ?1 AND ended_at = ''",
                    params![id],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO history_epochs (
                         epoch_kind, binary_path, binary_mtime, version,
                         started_at, ended_at, pid, exe_deleted, recorded_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        epoch.epoch_kind,
                        epoch.binary_path,
                        epoch.binary_mtime,
                        epoch.version,
                        epoch.started_at,
                        epoch.ended_at,
                        epoch.pid,
                        i64::from(epoch.exe_deleted),
                        now,
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };

        tx.commit()?;
        Ok(id)
    }

    fn touch_epoch_end(&self, pid: i64, ended_at: &str) -> Result<bool> {
        let conn = self.lock();
        let updated = conn.execute(
            "UPDATE history_epochs
                SET ended_at = ?2
              WHERE id = (SELECT id FROM history_epochs
                           WHERE epoch_kind = ?3 AND pid = ?1
                           ORDER BY started_at DESC LIMIT 1)",
            params![pid, ended_at, EPOCH_KIND_DAEMON_START],
        )?;
        Ok(updated > 0)
    }

    fn list_epochs(&self, since: Option<&str>, limit: usize) -> Result<Vec<HistoryEpoch>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, epoch_kind, binary_path, binary_mtime, version, started_at,
                    ended_at, pid, exe_deleted, recorded_at
               FROM history_epochs
              WHERE (?1 IS NULL OR datetime(started_at) >= datetime(?1))
              ORDER BY datetime(started_at) ASC, id ASC
              LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![since, limit.max(1) as i64], |row| {
                Ok(HistoryEpoch {
                    id: row.get(0)?,
                    epoch_kind: row.get(1)?,
                    binary_path: row.get(2)?,
                    binary_mtime: row.get(3)?,
                    version: row.get(4)?,
                    started_at: row.get(5)?,
                    ended_at: row.get(6)?,
                    pid: row.get(7)?,
                    exe_deleted: row.get::<_, i64>(8)? != 0,
                    recorded_at: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn backfill_epochs_from_daemons(&self) -> Result<EpochBackfill> {
        let mut conn = self.lock();
        let has_source: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
                            WHERE type = 'table' AND name = 'daemon_instances')",
            [],
            |row| row.get(0),
        )?;
        if !has_source {
            return Ok(EpochBackfill::default());
        }

        let now = chrono::Utc::now().to_rfc3339();
        let tx = conn.transaction()?;
        let rows: Vec<(i64, String, String)> = {
            let mut stmt =
                tx.prepare("SELECT pid, started_at, last_heartbeat FROM daemon_instances")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut out = EpochBackfill {
            source_available: true,
            scanned: rows.len() as i64,
            ..EpochBackfill::default()
        };
        {
            let mut insert = tx.prepare(
                "INSERT INTO history_epochs (
                     epoch_kind, binary_path, binary_mtime, version,
                     started_at, ended_at, pid, exe_deleted, recorded_at
                 ) VALUES (?1, NULL, NULL, NULL, ?2, ?3, ?4, 0, ?5)",
            )?;
            let mut exists = tx.prepare(
                "SELECT EXISTS(SELECT 1 FROM history_epochs
                                WHERE epoch_kind = ?1 AND COALESCE(pid, -1) = ?2
                                  AND started_at = ?3)",
            )?;
            for (pid, started_at, last_heartbeat) in rows {
                let present: bool = exists.query_row(
                    params![EPOCH_KIND_DAEMON_START, pid, started_at],
                    |row| row.get(0),
                )?;
                if present {
                    out.already_present += 1;
                    continue;
                }
                // `last_heartbeat` equals `started_at` for a daemon that never
                // ticked; that is still an observation of liveness at that
                // instant, so it is kept rather than nulled.
                insert.execute(params![
                    EPOCH_KIND_DAEMON_START,
                    started_at,
                    last_heartbeat,
                    pid,
                    now
                ])?;
                out.inserted += 1;
            }
        }
        tx.commit()?;
        Ok(out)
    }

    fn observation_counts(
        &self,
        from: &str,
        until: Option<&str>,
        symptom: Option<&str>,
    ) -> Result<ObservationCounts> {
        let conn = self.lock();
        let has_events: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
                            WHERE type = 'table' AND name = 'events')",
            [],
            |row| row.get(0),
        )?;
        if !has_events {
            return Ok(ObservationCounts::default());
        }

        let row = conn.query_row(
            "SELECT COUNT(*),
                    SUM(CASE WHEN ?3 IS NULL
                              OR event_type LIKE '%' || ?3 || '%'
                              OR summary LIKE '%' || ?3 || '%'
                             THEN 1 ELSE 0 END)
               FROM events
              WHERE datetime(created_at) >= datetime(?1)
                AND (?2 IS NULL OR datetime(created_at) < datetime(?2))",
            params![from, until, symptom],
            |row| {
                Ok(ObservationCounts {
                    sample: row.get(0)?,
                    matches: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                })
            },
        )?;
        Ok(row)
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

    // ---- Binary epochs (spec §9, EPIC cas-6212 / cas-8d2a, M8) ----

    fn epoch(started: &str, pid: i64) -> HistoryEpoch {
        HistoryEpoch {
            id: 0,
            epoch_kind: EPOCH_KIND_DAEMON_START.into(),
            binary_path: Some("/usr/local/bin/cas".into()),
            binary_mtime: Some("2026-08-07T20:55:00Z".into()),
            version: Some("2.49.0".into()),
            started_at: started.into(),
            ended_at: Some(started.into()),
            pid: Some(pid),
            exe_deleted: false,
            recorded_at: started.into(),
        }
    }

    /// AC(1): a daemon start writes an epoch, and writing the same start twice
    /// refreshes one row rather than inventing a second window for the same
    /// process — a duplicate would widen MIXED and suppress CLEAN-POST.
    #[test]
    fn record_epoch_is_idempotent_per_process_start() {
        let (_t, store) = store();
        let first = store.record_epoch(&epoch("2026-08-07T21:02:26Z", 42)).unwrap();
        let second = store.record_epoch(&epoch("2026-08-07T21:02:26Z", 42)).unwrap();
        assert_eq!(first, second);
        assert_eq!(store.list_epochs(None, 100).unwrap().len(), 1);

        store.record_epoch(&epoch("2026-08-07T21:40:00Z", 43)).unwrap();
        let all = store.list_epochs(None, 100).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].started_at, "2026-08-07T21:02:26Z", "ordered by start");
        assert_eq!(all[0].version.as_deref(), Some("2.49.0"));
        assert_eq!(all[0].binary_mtime.as_deref(), Some("2026-08-07T20:55:00Z"));
    }

    #[test]
    fn touch_epoch_end_advances_the_liveness_stamp() {
        let (_t, store) = store();
        store.record_epoch(&epoch("2026-08-07T21:02:26Z", 42)).unwrap();
        assert!(store.touch_epoch_end(42, "2026-08-07T21:36:35Z").unwrap());
        assert_eq!(
            store.list_epochs(None, 10).unwrap()[0].ended_at.as_deref(),
            Some("2026-08-07T21:36:35Z")
        );
        assert!(
            !store.touch_epoch_end(999, "2026-08-07T21:36:35Z").unwrap(),
            "an unknown pid must report that nothing was updated"
        );
    }

    /// A refresh that carries no liveness stamp must not erase one already
    /// observed — the tail of an old daemon is the whole MIXED window.
    #[test]
    fn re_recording_never_rewinds_ended_at() {
        let (_t, store) = store();
        store.record_epoch(&epoch("2026-08-07T21:02:26Z", 42)).unwrap();
        store.touch_epoch_end(42, "2026-08-07T21:36:35Z").unwrap();

        let mut stale = epoch("2026-08-07T21:02:26Z", 42);
        stale.ended_at = None;
        store.record_epoch(&stale).unwrap();

        assert_eq!(
            store.list_epochs(None, 10).unwrap()[0].ended_at.as_deref(),
            Some("2026-08-07T21:36:35Z")
        );
    }

    /// AC(2): historical epochs come out of `daemon_instances`, twice safely.
    #[test]
    fn backfill_reads_daemon_instances_and_repeats_safely() {
        let (_t, store) = store();
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE daemon_instances (
                     id TEXT PRIMARY KEY, pid INTEGER NOT NULL, daemon_type TEXT NOT NULL,
                     started_at TEXT NOT NULL, last_heartbeat TEXT NOT NULL, status TEXT NOT NULL);
                 INSERT INTO daemon_instances VALUES
                     ('d1', 111, 'mcp_embedded', '2026-08-07T19:00:00Z', '2026-08-07T21:36:35Z', 'running'),
                     ('d2', 222, 'mcp_embedded', '2026-08-07T21:02:26Z', '2026-08-07T23:00:00Z', 'running');",
            )
            .unwrap();
        }

        let first = store.backfill_epochs_from_daemons().unwrap();
        assert!(first.source_available);
        assert_eq!((first.scanned, first.inserted, first.already_present), (2, 2, 0));

        let second = store.backfill_epochs_from_daemons().unwrap();
        assert_eq!((second.scanned, second.inserted, second.already_present), (2, 0, 2));

        let epochs = store.list_epochs(None, 100).unwrap();
        assert_eq!(epochs.len(), 2);
        assert_eq!(epochs[0].ended_at.as_deref(), Some("2026-08-07T21:36:35Z"));
        assert!(
            epochs[0].binary_mtime.is_none() && epochs[0].version.is_none(),
            "daemon_instances records no binary identity, and the backfill must not invent one"
        );
    }

    /// No `daemon_instances` table is "unmeasurable", not "zero daemons".
    #[test]
    fn backfill_reports_a_missing_source_rather_than_an_empty_one() {
        let (_t, store) = store();
        let out = store.backfill_epochs_from_daemons().unwrap();
        assert!(!out.source_available);
        assert_eq!(out.scanned, 0);
    }

    #[test]
    fn observation_counts_window_and_symptom() {
        let (_t, store) = store();
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE events (
                     id INTEGER PRIMARY KEY AUTOINCREMENT, event_type TEXT NOT NULL,
                     entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, summary TEXT NOT NULL,
                     metadata TEXT, created_at TEXT NOT NULL, session_id TEXT);
                 INSERT INTO events (event_type, entity_type, entity_id, summary, created_at) VALUES
                     ('wake_missed', 'agent', 'a', 'worker did not wake', '2026-08-07T20:00:00Z'),
                     ('task_created', 'task', 't', 'created', '2026-08-07T21:10:00+00:00'),
                     ('wake_missed', 'agent', 'a', 'worker did not wake', '2026-08-07T21:20:00Z'),
                     ('task_created', 'task', 't', 'created', '2026-08-07T22:00:00Z'),
                     ('task_created', 'task', 't', 'created', '2026-08-07T22:30:00Z');",
            )
            .unwrap();
        }

        // The MIXED window: 21:02:26 → 21:36:35. One symptom, two observations.
        let mixed = store
            .observation_counts(
                "2026-08-07T21:02:26Z",
                Some("2026-08-07T21:36:35Z"),
                Some("wake_missed"),
            )
            .unwrap();
        assert_eq!(mixed, ObservationCounts { matches: 1, sample: 2 });

        // CLEAN-POST: quiet of the symptom, two observations of sample.
        let post = store
            .observation_counts("2026-08-07T21:36:35Z", None, Some("wake_missed"))
            .unwrap();
        assert_eq!(post, ObservationCounts { matches: 0, sample: 2 });

        // No symptom filter: every row in the window is a match.
        let all = store
            .observation_counts("2026-08-07T21:36:35Z", None, None)
            .unwrap();
        assert_eq!(all, ObservationCounts { matches: 2, sample: 2 });
    }

    /// A `…Z` stamp and a `…+00:00` stamp for the same instant must land in the
    /// same window; string comparison alone would not guarantee that.
    #[test]
    fn observation_counts_normalizes_timestamp_formats() {
        let (_t, store) = store();
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE events (
                     id INTEGER PRIMARY KEY AUTOINCREMENT, event_type TEXT NOT NULL,
                     entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, summary TEXT NOT NULL,
                     metadata TEXT, created_at TEXT NOT NULL, session_id TEXT);
                 INSERT INTO events (event_type, entity_type, entity_id, summary, created_at) VALUES
                     ('e', 'x', 'y', 's', '2026-08-07T21:59:59.123456789+00:00'),
                     ('e', 'x', 'y', 's', '2026-08-07T22:00:01Z'),
                     ('e', 'x', 'y', 's', '2026-08-08T00:00:00+02:00');",
            )
            .unwrap();
        }
        // 22:00:00Z onwards: the second row, plus the +02:00 row (= 22:00:00Z).
        let counts = store
            .observation_counts("2026-08-07T22:00:00Z", None, None)
            .unwrap();
        assert_eq!(counts.sample, 2);
    }

    /// No `events` table at all is zero observations, which the verdict layer
    /// then reads as INSUFFICIENT — never as a quiet, verified window.
    #[test]
    fn observation_counts_tolerate_a_missing_events_table() {
        let (_t, store) = store();
        assert_eq!(
            store
                .observation_counts("2026-08-07T21:36:35Z", None, Some("x"))
                .unwrap(),
            ObservationCounts::default()
        );
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
