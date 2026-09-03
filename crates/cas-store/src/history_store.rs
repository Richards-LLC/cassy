//! Structural git-history index (EPIC cas-6212 / cas-7a21 + cas-0562, spec §4).
//!
//! Five tables, all project-scoped:
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

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::history_provenance::{
    CandidateClass, CommitProvenance, EdgeHealth, FULL_SHA_LEN, LINK_METHOD_FACTORY_ANCHOR,
    LINK_METHOD_HOOK_OBSERVED, LINK_METHOD_TASK_NOTE, LINK_METHOD_WORKER_EVENT_EXACT,
    LINK_METHOD_WORKER_EVENT_PREFIX, LinkConfidence, ProvenanceLink, candidate_matches,
    classify_candidate,
};
use crate::shared_db;

/// Canonical DDL for the history subsystem, in `execute_batch` form.
/// `embedding_error` is declared LAST on purpose: an upgraded store receives it
/// from an `ALTER TABLE ... ADD COLUMN` (m250), which can only append, and m224's
/// shape guard compares column ORDER between a migrated store and a fresh one.
/// Keep any new column at the end, and keep comments out of the DDL text —
/// SQLite re-parses the stored `CREATE TABLE` on `DROP COLUMN` and fails on one.
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
    symbol_mapping TEXT NOT NULL DEFAULT 'pending',
    embedding_error TEXT
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
        scope TEXT NOT NULL DEFAULT 'project',
        symbol_mapping TEXT NOT NULL DEFAULT 'pending',
        embedding_error TEXT
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
/// `embedding_error` is declared LAST on purpose: an upgraded store receives it
/// from an `ALTER TABLE ... ADD COLUMN` (m250), which can only append, and m224's
/// shape guard compares column ORDER between a migrated store and a fresh one.
/// Keep any new column at the end, and keep comments out of the DDL text —
/// SQLite re-parses the stored `CREATE TABLE` on `DROP COLUMN` and fails on one.
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
    scope TEXT NOT NULL DEFAULT 'project',
    embedding_error TEXT
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
        scope TEXT NOT NULL DEFAULT 'project',
        embedding_error TEXT
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
/// Keep in lockstep; `m228`'s shape-drift test fails on any divergence.
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

/// The `history_index_state.source` value for the embedding drain (M7).
///
/// The drain writes no rows and moves no watermark, so it owns a ledger row for
/// exactly one purpose: recording that it ran, and what went wrong when it did
/// not. Without it a failing drain is only observable as "the pending count is
/// not going down", which is the tracing::warn-only failure mode M7 exists to
/// end.
pub const SOURCE_EMBEDDINGS: &str = "embeddings";

/// Maximum number of incompletely mapped commits admitted to one
/// symbol-filtered result set. They are useful as explicit uncertainty, but an
/// unbounded uncertain tail can both swamp callers and hide exact mapped hits.
pub const HISTORY_SYMBOL_UNCERTAIN_LIMIT: usize = 10;

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
    /// How completely M3 could map this commit's changed files to symbols.
    /// `absent`, `pending`, and `partial` mean a symbol-filtered query cannot
    /// prove that the commit did not touch its requested symbol.
    pub symbol_mapping: String,
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
    /// Exact qualified symbol name recorded by M3 in
    /// `history_commit_symbols`. Rows whose mapping is incomplete are also
    /// returned so the caller can report uncertainty rather than manufacture a
    /// false negative.
    pub symbol: Option<String>,
    /// Inclusive lower bound on `committed_at` (RFC3339).
    pub since: Option<String>,
    /// Inclusive upper bound on `committed_at` (RFC3339).
    pub until: Option<String>,
    /// Merge commits are indexed structurally but carry `Merge branch 'x'` as
    /// their whole message (spec §7.1), so they are noise in a text search.
    /// Off by default; callers asking a structural question can turn them on.
    pub include_merges: bool,
    /// Restrict the answer to this exact SHA set (EPIC cas-6212 / cas-519f).
    ///
    /// This is how the `task_id` / `session_id` filters are honoured: the
    /// provenance resolver produces the SHA set, and it is applied **in SQL,
    /// before `LIMIT`**. Post-filtering the ranked page instead would silently
    /// return fewer than `limit` results — or none — whenever the task's commits
    /// were not already in the top-k, which reads as "that task shipped
    /// nothing".
    ///
    /// `Some(empty)` means "the filter resolved to no commits" and correctly
    /// matches nothing; `None` means "no filter".
    pub shas: Option<Vec<String>>,
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
/// many commits carry each edge that is populated, and what each edge had to
/// throw away to say so.
///
/// M5 (cas-519f) widened it from one number to a ledger. A single percentage
/// cannot distinguish "the edge is thin" from "the edge is broken", and §5.2's
/// own 10× correction happened precisely because a row class had never been
/// re-counted.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Commits reachable through **any** populated edge, including the
    /// medium/low-confidence ones. Reported beside the high-confidence figure
    /// rather than instead of it — spec §10.1 asks for both numbers, split by
    /// confidence, so the debt stays visible even as the weaker edges fill in.
    pub any_confidence_linked: Option<i64>,
    pub any_coverage_pct: Option<f64>,
    /// Commits reachable per `link_method`, method-ascending. The breakdown a
    /// reader needs to tell "the anchor edge is growing" from "the text edge is
    /// matching more loosely".
    pub by_method: Vec<(String, i64)>,
    /// Per-edge usable/excluded counts (spec §5.2's row classes made queryable).
    pub edges: Vec<EdgeHealth>,
    /// Why the measurement could not be taken, when it could not.
    pub unmeasurable_reason: Option<String>,
}

/// How many indexed commits an ambiguity probe will name. Two is enough to
/// *decide* ambiguity; the extra room exists so the answer can say "at least
/// five" rather than implying it enumerated a colliding set it never counted.
const AMBIGUITY_PROBE_LIMIT: usize = 5;

/// Upper bound on the edges reported for a single commit.
///
/// An ambiguous 7-char prefix can be carried by dozens of events from different
/// sessions, and a caller reading a page of ten commits does not need a hundred
/// near-identical edges. Truncation is never silent: the count that was dropped
/// is stated in [`CommitProvenance::reason`].
const MAX_LINKS_PER_COMMIT: usize = 20;

/// A usable `worker_git_commit` row, already classified.
struct WorkerCommitEvent {
    head_sha: String,
    session_id: Option<String>,
    agent_id: Option<String>,
    created_at: Option<String>,
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
    /// shape during a release day. Always `None` when [`Self::exe_deleted`] is
    /// true: metadata at that path belongs to the replacement file, not the
    /// executable backing this process.
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

impl HistoryEpoch {
    /// Binary mtime that is safe to use as evidence about this process.
    ///
    /// Keep this guard at the model boundary as defense in depth for callers
    /// holding rows written before storage normalized stale executable
    /// identities to a NULL mtime.
    pub fn trusted_binary_mtime(&self) -> Option<&str> {
        if self.exe_deleted {
            None
        } else {
            self.binary_mtime.as_deref()
        }
    }
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

    /// Resolve `commit → task / session` provenance for a batch of SHAs
    /// (spec §5.2 — EPIC cas-6212 / cas-519f).
    ///
    /// **Batch, deliberately.** `events` is ~978 K rows on the live database
    /// with no index on `event_type` (only `created_at` and
    /// `(entity_type, entity_id)`), so the per-commit form of this query would
    /// full-scan a million rows once per result. One scan serves the whole
    /// page.
    ///
    /// Every requested SHA appears in the result. A commit with no populated
    /// edge comes back with an empty `links` and a stated `reason` — §5.2's
    /// "never a silent empty", and §6.4 Q3's explicit requirement that unlinked
    /// commits are returned rather than dropped.
    fn resolve_provenance(
        &self,
        repository: &str,
        shas: &[String],
    ) -> Result<HashMap<String, CommitProvenance>>;

    /// Indexed commits attributable to a task, strongest edge first
    /// (the `task_id` filter, spec §6.1).
    fn shas_for_task(&self, repository: &str, task_id: &str) -> Result<Vec<String>>;

    /// Indexed commits attributable to a session (the `session_id` filter).
    fn shas_for_session(&self, repository: &str, session_id: &str) -> Result<Vec<String>>;

    /// Indexed commits that carry no `commit_links` row yet — the work list for
    /// the spine repair (spec §5.3). Newest first, so a bounded pass repairs the
    /// commits most likely to be asked about.
    ///
    /// `offset` exists because most commits on this corpus will *never* get a
    /// link: 89.7% of `worker_git_commit` rows carry no SHA, and the anchor edge
    /// names a task rather than a session. Without it a "repair everything" loop
    /// re-reads the same unresolvable head of the list on every pass and
    /// advances only by the number of links it wrote — which on real data means
    /// it stops long before reaching the older commits. The caller advances the
    /// offset by the number of rows that *stayed* unlinked.
    fn commits_without_links(
        &self,
        repository: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>>;

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

    // === Embedding queue (M7, spec §4.4) ===
    //
    // Store-wide rather than per-repository on purpose: the drain is a
    // background arm that must empty the whole queue this database carries, and
    // a per-repository list would silently strand rows belonging to any
    // repository the caller did not think to name.

    /// Commits still awaiting a vector, oldest first, capped at `limit`.
    fn list_pending_embedding_commits(&self, limit: usize) -> Result<Vec<HistoryCommit>>;

    /// Docs still awaiting a vector, oldest first, capped at `limit`.
    fn list_pending_embedding_docs(&self, limit: usize) -> Result<Vec<HistoryDoc>>;

    /// Clear `pending_embedding` for a commit whose vector is now cached.
    fn mark_commit_embedded(&self, sha: &str) -> Result<()>;

    /// Clear `pending_embedding` for a doc whose vector is now cached.
    fn mark_doc_embedded(&self, id: &str) -> Result<()>;

    /// Clear `pending_embedding` for a commit that will **never** be embedded.
    ///
    /// Distinct from [`Self::mark_commit_embedded`] because the two facts are
    /// different and the caller's report says so: a merge commit excluded per
    /// spec §12 Q5 has no vector and never will, but it is also not *awaiting*
    /// one. Leaving it armed would park 32% of this repo's commits permanently
    /// at the head of the queue, and "pending drains to zero" — the property
    /// M7 is judged on — could never hold.
    fn skip_commit_embedding(&self, sha: &str) -> Result<()>;

    /// `(commits, docs)` still awaiting a vector, store-wide.
    fn count_pending_embedding(&self) -> Result<(i64, i64)>;

    /// Retire a unit the provider refused, keeping the refusal on the row.
    ///
    /// Distinct from both [`Self::mark_commit_embedded`] and
    /// [`Self::skip_commit_embedding`]: the unit has no vector, was not
    /// excluded by policy, and retrying the identical payload cannot change the
    /// answer (GH #695: a 138k-char commit body over the model's token cap).
    /// Leaving it pending pins the whole queue behind it; clearing it silently
    /// loses the only evidence of why the corpus is incomplete. Quarantine
    /// removes it from `pending` and keeps the provider's message for
    /// [`Self::count_quarantined_embedding`] and the retry path.
    fn quarantine_commit_embedding(&self, sha: &str, error: &str) -> Result<()>;

    /// [`Self::quarantine_commit_embedding`] for a doc.
    fn quarantine_doc_embedding(&self, id: &str, error: &str) -> Result<()>;

    /// `(commits, docs)` retired with a provider refusal recorded.
    fn count_quarantined_embedding(&self) -> Result<(i64, i64)>;

    /// The most recent provider refusal recorded on any quarantined unit.
    ///
    /// Reporting the count without the reason gives an operator a number and no
    /// move; this is the sentence that names what the provider actually said.
    fn last_quarantined_embedding_error(&self) -> Result<Option<String>>;

    /// Re-arm every quarantined unit and return how many were requeued.
    ///
    /// The operator escape hatch: a provider cap that has been raised, or a
    /// client-side cap that now truncates the payload, makes a previously
    /// refused unit embeddable again.
    fn requeue_quarantined_embeddings(&self) -> Result<usize>;

    /// Re-arm every commit and doc for embedding.
    ///
    /// Called when the embedding model changes: vectors from two models are not
    /// comparable, so the cache is dropped and the whole corpus must be
    /// recomputed — not just the rows that happened to be pending.
    fn mark_all_pending_embedding(&self) -> Result<()>;

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

    /// Shared body of "this commit is no longer awaiting a vector", whether
    /// because it got one or because it never will (M7).
    /// Retire a commit from the embedding queue.
    ///
    /// Clears `embedding_error` too: a unit that just succeeded (or was
    /// excluded by policy) is not quarantined any more, and a stale refusal
    /// would keep counting against the corpus forever.
    fn clear_commit_pending(&self, sha: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE history_commits SET pending_embedding = 0, embedding_error = NULL WHERE sha = ?1",
            params![sha],
        )?;
        Ok(())
    }

    /// Columns of `history_commits`, named explicitly so a `SELECT c.*` in a
    /// join cannot shift positions under the reader.
    const COMMIT_COLUMNS: &'static str = "c.sha, c.short_sha, c.parent_shas, c.is_merge, \
         c.author_name, c.author_email, c.authored_at, c.committed_at, c.subject, c.body, \
         c.branch_hint, c.repository, c.symbol_mapping";

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
            symbol_mapping: row.get("symbol_mapping")?,
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
            idx += 1;
        }
        if let Some(symbol) = &query.symbol {
            // A symbol row is positive evidence, but an incomplete M3 verdict
            // is not negative evidence. Excluding `pending`, `absent`, or
            // `partial` commits here would turn index lag into a confident
            // claim that the named symbol was never touched. Return those rows
            // with their verdict; `history::search` serializes it per hit.
            sql.push_str(&format!(
                " AND (c.symbol_mapping IN ('pending', 'absent', 'partial') \
                   OR EXISTS (SELECT 1 FROM history_commit_symbols s \
                              WHERE s.sha = c.sha AND s.qualified_name = ?{idx}))"
            ));
            params.push(symbol.clone());
            idx += 1;
        }
        if let Some(shas) = &query.shas {
            if shas.is_empty() {
                // The filter resolved to nothing. `AND 0` is the honest
                // encoding: an empty `IN ()` is a syntax error, and skipping
                // the clause would widen "commits from this task" to "every
                // commit", which is the worst possible failure for a filter.
                sql.push_str(" AND 0");
            } else {
                let placeholders = (idx..idx + shas.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!(" AND c.sha IN ({placeholders})"));
                params.extend(shas.iter().cloned());
            }
        }
        (sql, params)
    }

    /// Is `name` a real table in this database?
    ///
    /// Every provenance edge lives in another subsystem's table. A store opened
    /// without them is legitimate (a fresh project; the history tables under
    /// test), and the difference between "this edge is empty" and "this edge
    /// cannot be read here" is exactly the difference §10.1 refuses to collapse.
    fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![name],
            |row| row.get(0),
        )
    }

    /// Every usable `metadata.head_sha` from `events.worker_git_commit`, with
    /// the row classes it had to exclude counted rather than dropped.
    ///
    /// ONE scan. `events` carries ~978 K rows on the live database and has no
    /// index on `event_type` (`event_store.rs` creates only `created_at` and
    /// `(entity_type, entity_id)`), so this is the expensive part of provenance
    /// and it is paid once per call, never once per commit.
    fn worker_commit_events(conn: &Connection) -> rusqlite::Result<(Vec<WorkerCommitEvent>, EdgeHealth)> {
        let mut health = EdgeHealth {
            edge: LINK_METHOD_WORKER_EVENT_PREFIX.to_string(),
            ..Default::default()
        };
        let mut stmt = conn.prepare(
            "SELECT json_extract(metadata, '$.head_sha') AS head_sha,
                    COALESCE(session_id, '') AS session_id,
                    COALESCE(entity_id, '')  AS entity_id,
                    created_at
               FROM events
              WHERE event_type = 'worker_git_commit'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>("head_sha")?,
                row.get::<_, String>("session_id")?,
                row.get::<_, String>("entity_id")?,
                row.get::<_, Option<String>>("created_at")?,
            ))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (head_sha, session_id, entity_id, created_at) = row?;
            let Some(head_sha) = head_sha else {
                health.excluded_absent += 1;
                continue;
            };
            match classify_candidate(&head_sha) {
                CandidateClass::Exact | CandidateClass::Prefix => {}
                CandidateClass::Stub => {
                    health.excluded_stub += 1;
                    continue;
                }
                CandidateClass::TooShort | CandidateClass::Invalid => {
                    health.excluded_unusable += 1;
                    continue;
                }
            }
            health.usable_rows += 1;
            events.push(WorkerCommitEvent {
                head_sha: head_sha.trim().to_string(),
                session_id: (!session_id.is_empty()).then_some(session_id),
                agent_id: (!entity_id.is_empty()).then_some(entity_id),
                created_at,
            });
        }

        let distinct: std::collections::HashSet<&str> =
            events.iter().map(|e| e.head_sha.as_str()).collect();
        health.distinct_identifiers = distinct.len() as i64;
        Ok((events, health))
    }

    /// Indexed commits a prefix matches, bounded. Used to decide ambiguity —
    /// which is a *measured* property of this repository's index, never inferred
    /// from the prefix's length.
    fn commits_matching_prefix(
        conn: &Connection,
        repository: &str,
        prefix: &str,
    ) -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare_cached(
            "SELECT sha FROM history_commits
              WHERE repository = ?1 AND sha LIKE ?2 || '%'
              LIMIT ?3",
        )?;
        stmt.query_map(
            params![repository, prefix, AMBIGUITY_PROBE_LIMIT as i64],
            |row| row.get(0),
        )?
        .collect()
    }

    /// Full 40-char object names mentioned in a blob of free text.
    ///
    /// Used for the `tasks.notes` edge in the *task → commits* direction, where
    /// the note is the haystack. Only full SHAs are extracted: an 8-char hex run
    /// in prose is as likely to be a timestamp fragment as a commit, and this
    /// direction has no second edge to cross-check it against.
    fn full_shas_in(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_hexdigit() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == FULL_SHA_LEN {
                out.push(bytes[start..i].iter().collect::<String>().to_lowercase());
            }
        }
        out.sort();
        out.dedup();
        out
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

        // `CREATE TABLE IF NOT EXISTS` is a no-op on a store that M1 already
        // created, so M3's added column needs its own idempotent ALTER. The
        // numbered migration (m224) does this too; doing it here as well keeps
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
        // Same idempotent-ALTER reasoning as `symbol_mapping` above. A unit the
        // provider refuses needs a state that is neither "pending" nor
        // "embedded": without it a rejected unit can only be retried forever
        // (the GH #695 shape) or cleared silently, and neither is reportable.
        for table in ["history_commits", "history_docs"] {
            let has_column = conn
                .prepare(&format!(
                    "SELECT 1 FROM pragma_table_info('{table}') WHERE name = 'embedding_error'"
                ))?
                .exists([])?;
            if !has_column {
                conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN embedding_error TEXT"))?;
            }
        }
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

        // Every mapped candidate admitted by `filter_sql` has an exact symbol
        // row. Put that certain tier ahead of pending/absent/partial rows
        // before LIMIT; recency and FTS relevance only order within a tier.
        let certainty_order = query
            .symbol
            .as_ref()
            .map(|_| "CASE WHEN c.symbol_mapping = 'mapped' THEN 0 ELSE 1 END, ")
            .unwrap_or_default();

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
                          ORDER BY {certainty_order}score
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
                          ORDER BY {certainty_order}c.committed_at DESC, c.sha
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
        if query.symbol.is_some() {
            // The hybrid layer sorts this score again. Preserve the SQL
            // certainty tier across that second ranking step, then retain a
            // small, explicit tail of unknowns rather than either hiding them
            // or returning an unbounded maybe-list.
            let uncertain_max = hits
                .iter()
                .filter(|hit| hit.commit.symbol_mapping != "mapped")
                .map(|hit| hit.score)
                .fold(0.0_f64, f64::max);
            for hit in &mut hits {
                if hit.commit.symbol_mapping == "mapped" {
                    hit.score += uncertain_max + 1.0;
                }
            }
            let mut uncertain = 0;
            hits.retain(|hit| {
                if hit.commit.symbol_mapping == "mapped" {
                    true
                } else if uncertain < HISTORY_SYMBOL_UNCERTAIN_LIMIT {
                    uncertain += 1;
                    true
                } else {
                    false
                }
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

        // The edges live in other subsystems' tables. A store that has never
        // seen a task is a legitimate state (a fresh project, or the history
        // tables opened standalone in a test), so its absence is reported as
        // "not measurable" rather than as 0% — which would read as a finding.
        let has_tasks = Self::table_exists(&conn, "tasks")?;
        let has_events = Self::table_exists(&conn, "events")?;
        let has_links = Self::table_exists(&conn, "commit_links")?;
        if !has_tasks && !has_events && !has_links {
            return Ok(ProvenanceCoverage {
                total_commits,
                unmeasurable_reason: Some(
                    "no tasks, events or commit_links table in this store: no provenance edge \
                     can be read here"
                        .to_string(),
                ),
                ..Default::default()
            });
        }

        let mut by_method: Vec<(String, i64)> = Vec::new();
        let mut edges: Vec<EdgeHealth> = Vec::new();
        let mut linked_any: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut high_confidence: Option<i64> = None;

        // --- Edge 1: the exact anchor. The high-confidence figure IS this one.
        if has_tasks {
            // The anchors are materialized once and joined, rather than being
            // recomputed inside a correlated `EXISTS` per commit. The
            // correlated form parses every task's `deliverables` JSON once per
            // (commit, task) pair: on the GH #700 store — 8,509 commits,
            // 4,465 tasks, kilobyte payloads — that single query measured
            // 77.6s, and `cas doctor` spent 121s of 126s in this function.
            // Same rows, same count, 0.02s (cas-ba01).
            let mut stmt = conn.prepare(
                "WITH anchors AS (
                     SELECT DISTINCT
                            json_extract(deliverables, '$.factory_branch_anchor') AS anchor
                       FROM tasks
                      WHERE json_extract(deliverables, '$.factory_branch_anchor') IS NOT NULL
                 )
                 SELECT c.sha FROM history_commits c
                   JOIN anchors a ON a.anchor = c.sha
                  WHERE c.repository = ?1",
            )?;
            let anchored: Vec<String> = stmt
                .query_map(params![repository], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            // One pass over `tasks` for both figures, for the same reason.
            let (anchor_total, anchor_distinct): (i64, i64) = conn.query_row(
                "SELECT COUNT(*),
                        COUNT(DISTINCT json_extract(deliverables, '$.factory_branch_anchor'))
                   FROM tasks
                  WHERE json_extract(deliverables, '$.factory_branch_anchor') IS NOT NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            high_confidence = Some(anchored.len() as i64);
            by_method.push((LINK_METHOD_FACTORY_ANCHOR.to_string(), anchored.len() as i64));
            edges.push(EdgeHealth {
                edge: LINK_METHOD_FACTORY_ANCHOR.to_string(),
                usable_rows: anchor_total,
                distinct_identifiers: anchor_distinct,
                ..Default::default()
            });
            linked_any.extend(anchored);
        }

        // --- Edge 2: the variable-width event prefixes.
        //
        // Matched in memory rather than in SQL. The alternative — a correlated
        // `LIKE` over `history_commits` × the event rows — is a cross product,
        // and the memory here is bounded by the number of *indexed commits*
        // (~2.5 K on this repo, ~100 KB of SHA text), which the index already
        // holds row-for-row anyway.
        if has_events {
            let (events, health) = Self::worker_commit_events(&conn)?;
            let mut by_len: HashMap<usize, std::collections::HashSet<String>> = HashMap::new();
            for event in &events {
                by_len
                    .entry(event.head_sha.len())
                    .or_default()
                    .insert(event.head_sha.to_lowercase());
            }

            let mut stmt =
                conn.prepare("SELECT sha FROM history_commits WHERE repository = ?1")?;
            let all_shas: Vec<String> = stmt
                .query_map(params![repository], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;

            let mut matched = 0i64;
            for sha in &all_shas {
                let lower = sha.to_lowercase();
                let hit = by_len
                    .iter()
                    .any(|(len, set)| lower.len() >= *len && set.contains(&lower[..*len]));
                if hit {
                    matched += 1;
                    linked_any.insert(sha.clone());
                }
            }
            by_method.push((LINK_METHOD_WORKER_EVENT_PREFIX.to_string(), matched));
            edges.push(health);
        }

        // --- Edge 3: the repaired spine itself.
        if has_links {
            let mut stmt = conn.prepare(
                "SELECT c.sha FROM history_commits c
                  JOIN commit_links l ON l.commit_hash = c.sha
                 WHERE c.repository = ?1",
            )?;
            let linked: Vec<String> = stmt
                .query_map(params![repository], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let total_links: i64 =
                conn.query_row("SELECT COUNT(*) FROM commit_links", [], |row| row.get(0))?;
            by_method.push(("commit_links".to_string(), linked.len() as i64));
            edges.push(EdgeHealth {
                edge: "commit_links".to_string(),
                usable_rows: total_links,
                distinct_identifiers: total_links,
                ..Default::default()
            });
            linked_any.extend(linked);
        }

        by_method.sort();
        let any_linked = linked_any.len() as i64;
        let pct = |n: i64| (total_commits > 0).then(|| n as f64 * 100.0 / total_commits as f64);

        Ok(ProvenanceCoverage {
            total_commits,
            coverage_pct: high_confidence.and_then(pct),
            high_confidence_linked: high_confidence,
            any_confidence_linked: Some(any_linked),
            any_coverage_pct: pct(any_linked),
            by_method,
            edges,
            // The `tasks.notes` text edge is deliberately NOT counted here, and
            // saying so is part of the measurement. Counting it means a
            // `LIKE '%…%'` of every commit against every task's free-text notes
            // — O(commits × tasks × note length) with no index available — and
            // it is the one edge whose matches are substring coincidences as
            // often as they are provenance. It stays available per-commit in
            // `resolve_provenance`, at medium confidence, where a reader can
            // judge it; it does not inflate a headline percentage.
            unmeasurable_reason: (!has_tasks || !has_events || !has_links).then(|| {
                let mut missing = Vec::new();
                if !has_tasks {
                    missing.push("tasks");
                }
                if !has_events {
                    missing.push("events");
                }
                if !has_links {
                    missing.push("commit_links");
                }
                format!(
                    "partial measurement: {} not present in this store, so the edge(s) it \
                     carries are absent from these counts",
                    missing.join(", ")
                )
            }),
        })
    }

    fn resolve_provenance(
        &self,
        repository: &str,
        shas: &[String],
    ) -> Result<HashMap<String, CommitProvenance>> {
        let mut out: HashMap<String, CommitProvenance> = shas
            .iter()
            .map(|sha| {
                (
                    sha.clone(),
                    CommitProvenance {
                        sha: sha.clone(),
                        links: Vec::new(),
                        reason: None,
                    },
                )
            })
            .collect();
        if shas.is_empty() {
            return Ok(out);
        }

        let conn = self.lock();
        let has_tasks = Self::table_exists(&conn, "tasks")?;
        let has_events = Self::table_exists(&conn, "events")?;
        let has_links = Self::table_exists(&conn, "commit_links")?;

        // --- Edge: an existing commit_links row. Strongest when it was
        // OBSERVED (the PostToolUse hook watched the commit happen); merely
        // reconstructed rows carry the indexer's own link_method and are
        // reported at medium, so §5.3's "never confused with an observed one"
        // holds at read time and not only at write time.
        if has_links {
            let has_method = conn
                .prepare("SELECT 1 FROM pragma_table_info('commit_links') WHERE name = 'link_method'")?
                .exists([])?;
            let sql = if has_method {
                "SELECT commit_hash, session_id, agent_id, committed_at, link_method
                   FROM commit_links WHERE commit_hash = ?1"
            } else {
                "SELECT commit_hash, session_id, agent_id, committed_at, NULL AS link_method
                   FROM commit_links WHERE commit_hash = ?1"
            };
            let mut stmt = conn.prepare(sql)?;
            for sha in shas {
                let row = stmt
                    .query_row(params![sha], |row| {
                        Ok((
                            row.get::<_, String>("session_id")?,
                            row.get::<_, String>("agent_id")?,
                            row.get::<_, Option<String>>("committed_at")?,
                            row.get::<_, Option<String>>("link_method")?,
                        ))
                    })
                    .optional()?;
                let Some((session_id, agent_id, committed_at, link_method)) = row else {
                    continue;
                };
                // A row written before `link_method` existed is legacy hook
                // output: that path was the ONLY writer until this milestone.
                let method =
                    link_method.unwrap_or_else(|| LINK_METHOD_HOOK_OBSERVED.to_string());
                let confidence = if method == LINK_METHOD_HOOK_OBSERVED {
                    LinkConfidence::High
                } else {
                    LinkConfidence::Medium
                };
                if let Some(entry) = out.get_mut(sha) {
                    entry.links.push(ProvenanceLink {
                        link_method: method,
                        confidence,
                        task_id: None,
                        task_title: None,
                        session_id: (!session_id.is_empty()).then_some(session_id),
                        agent_id: (!agent_id.is_empty()).then_some(agent_id),
                        observed_at: committed_at,
                        matched_prefix: None,
                        ambiguous: false,
                        ambiguous_candidates: Vec::new(),
                    });
                }
            }
        }

        // --- Edge: the exact factory anchor (high confidence, no guard needed).
        if has_tasks {
            let mut stmt = conn.prepare(
                "SELECT id, title, updated_at, assignee FROM tasks
                  WHERE json_extract(deliverables, '$.factory_branch_anchor') = ?1",
            )?;
            for sha in shas {
                let rows = stmt.query_map(params![sha], |row| {
                    Ok((
                        row.get::<_, String>("id")?,
                        row.get::<_, Option<String>>("title")?,
                        row.get::<_, Option<String>>("updated_at")?,
                        row.get::<_, Option<String>>("assignee")?,
                    ))
                })?;
                for row in rows {
                    let (id, title, updated_at, assignee) = row?;
                    if let Some(entry) = out.get_mut(sha) {
                        entry.links.push(ProvenanceLink {
                            link_method: LINK_METHOD_FACTORY_ANCHOR.to_string(),
                            confidence: LinkConfidence::High,
                            task_id: Some(id),
                            task_title: title,
                            session_id: None,
                            agent_id: assignee,
                            observed_at: updated_at,
                            matched_prefix: None,
                            ambiguous: false,
                            ambiguous_candidates: Vec::new(),
                        });
                    }
                }
            }
        }

        // --- Edge: the variable-width worker_git_commit prefixes (§5.2).
        if has_events {
            let (events, _health) = Self::worker_commit_events(&conn)?;
            let mut ambiguity: HashMap<String, Vec<String>> = HashMap::new();
            // (sha, method, prefix, session) — one worker emits the same
            // session/head_sha pair on every session-stop, and a page of ten
            // commits does not need forty copies of the same edge.
            let mut seen: std::collections::HashSet<(String, String, String)> =
                std::collections::HashSet::new();

            for event in &events {
                let exact = classify_candidate(&event.head_sha) == CandidateClass::Exact;
                for sha in shas {
                    if !candidate_matches(&event.head_sha, sha) {
                        continue;
                    }
                    let key = (
                        sha.clone(),
                        event.head_sha.to_lowercase(),
                        event.session_id.clone().unwrap_or_default(),
                    );
                    if !seen.insert(key) {
                        continue;
                    }
                    // Ambiguity is asked of the index, once per prefix.
                    let candidates = if exact {
                        Vec::new()
                    } else {
                        match ambiguity.get(&event.head_sha) {
                            Some(c) => c.clone(),
                            None => {
                                let found = Self::commits_matching_prefix(
                                    &conn,
                                    repository,
                                    &event.head_sha,
                                )?;
                                ambiguity.insert(event.head_sha.clone(), found.clone());
                                found
                            }
                        }
                    };
                    let ambiguous = candidates.len() > 1;
                    if let Some(entry) = out.get_mut(sha) {
                        entry.links.push(ProvenanceLink {
                            link_method: if exact {
                                LINK_METHOD_WORKER_EVENT_EXACT.to_string()
                            } else {
                                LINK_METHOD_WORKER_EVENT_PREFIX.to_string()
                            },
                            // An ambiguous prefix is still returned — §5.2
                            // forbids silently picking a winner — but it is not
                            // allowed to look as good as an exact match.
                            confidence: match (exact, ambiguous) {
                                (true, _) => LinkConfidence::High,
                                (false, false) => LinkConfidence::High,
                                (false, true) => LinkConfidence::Low,
                            },
                            task_id: None,
                            task_title: None,
                            session_id: event.session_id.clone(),
                            agent_id: event.agent_id.clone(),
                            observed_at: event.created_at.clone(),
                            matched_prefix: Some(event.head_sha.clone()),
                            ambiguous,
                            ambiguous_candidates: if ambiguous { candidates } else { Vec::new() },
                        });
                    }
                }
            }
        }

        // --- Edge: the free-text close receipt in tasks.notes (medium).
        if has_tasks {
            let mut stmt = conn.prepare(
                "SELECT id, title, updated_at, assignee FROM tasks
                  WHERE notes LIKE '%' || ?1 || '%' LIMIT ?2",
            )?;
            for sha in shas {
                if sha.len() < FULL_SHA_LEN {
                    continue;
                }
                // The abbreviation, not the full SHA: close receipts quote both
                // forms, and the short one is a superstring test that catches
                // the long one too.
                let probe = &sha[..8];
                let already: std::collections::HashSet<String> = out
                    .get(sha)
                    .map(|p| p.links.iter().filter_map(|l| l.task_id.clone()).collect())
                    .unwrap_or_default();
                let rows = stmt.query_map(params![probe, MAX_LINKS_PER_COMMIT as i64], |row| {
                    Ok((
                        row.get::<_, String>("id")?,
                        row.get::<_, Option<String>>("title")?,
                        row.get::<_, Option<String>>("updated_at")?,
                        row.get::<_, Option<String>>("assignee")?,
                    ))
                })?;
                for row in rows {
                    let (id, title, updated_at, assignee) = row?;
                    // The anchor edge already named this task exactly; a
                    // substring hit on the same task adds no information and
                    // would pad the answer with fake corroboration.
                    if already.contains(&id) {
                        continue;
                    }
                    if let Some(entry) = out.get_mut(sha) {
                        entry.links.push(ProvenanceLink {
                            link_method: LINK_METHOD_TASK_NOTE.to_string(),
                            confidence: LinkConfidence::Medium,
                            task_id: Some(id),
                            task_title: title,
                            session_id: None,
                            agent_id: assignee,
                            observed_at: updated_at,
                            matched_prefix: Some(probe.to_string()),
                            ambiguous: false,
                            ambiguous_candidates: Vec::new(),
                        });
                    }
                }
            }
        }

        for provenance in out.values_mut() {
            // Strongest first, then stable by method so two runs agree.
            provenance.links.sort_by(|a, b| {
                b.confidence
                    .cmp(&a.confidence)
                    .then_with(|| a.link_method.cmp(&b.link_method))
                    .then_with(|| a.task_id.cmp(&b.task_id))
                    .then_with(|| a.session_id.cmp(&b.session_id))
            });
            if provenance.links.len() > MAX_LINKS_PER_COMMIT {
                let dropped = provenance.links.len() - MAX_LINKS_PER_COMMIT;
                provenance.links.truncate(MAX_LINKS_PER_COMMIT);
                provenance.reason = Some(format!(
                    "{MAX_LINKS_PER_COMMIT} strongest edges shown; {dropped} weaker edge(s) omitted"
                ));
            } else if provenance.links.is_empty() {
                provenance.reason = Some(match (has_tasks, has_events, has_links) {
                    (false, false, false) => "no provenance edge is readable in this store \
                         (no tasks, events or commit_links table)"
                        .to_string(),
                    _ => "no populated edge: this commit carries no factory anchor, no \
                          worker_git_commit event, no close-note mention and no commit_links row"
                        .to_string(),
                });
            }
        }

        Ok(out)
    }

    fn shas_for_task(&self, repository: &str, task_id: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        if !Self::table_exists(&conn, "tasks")? {
            return Ok(Vec::new());
        }
        let row = conn
            .query_row(
                "SELECT COALESCE(json_extract(deliverables, '$.factory_branch_anchor'), ''),
                        COALESCE(notes, '')
                   FROM tasks WHERE id = ?1",
                params![task_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((anchor, notes)) = row else {
            return Ok(Vec::new());
        };

        let mut candidates = Self::full_shas_in(&notes);
        if !anchor.is_empty() {
            candidates.push(anchor.to_lowercase());
        }
        candidates.sort();
        candidates.dedup();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Only SHAs this repository has actually indexed: a note can quote a
        // commit from another repo, and returning it would filter the answer
        // down to a commit the index cannot describe.
        let mut stmt = conn.prepare_cached(
            "SELECT sha FROM history_commits WHERE repository = ?1 AND sha = ?2",
        )?;
        let mut out = Vec::new();
        for candidate in candidates {
            if let Some(sha) = stmt
                .query_row(params![repository, candidate], |row| row.get::<_, String>(0))
                .optional()?
            {
                out.push(sha);
            }
        }
        Ok(out)
    }

    fn shas_for_session(&self, repository: &str, session_id: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();

        if Self::table_exists(&conn, "commit_links")? {
            let mut stmt = conn.prepare(
                "SELECT c.sha FROM history_commits c
                   JOIN commit_links l ON l.commit_hash = c.sha
                  WHERE c.repository = ?1 AND l.session_id = ?2",
            )?;
            let rows: Vec<String> = stmt
                .query_map(params![repository, session_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            out.extend(rows);
        }

        if Self::table_exists(&conn, "events")? {
            let mut stmt = conn.prepare(
                "SELECT json_extract(metadata, '$.head_sha') AS head_sha
                   FROM events
                  WHERE event_type = 'worker_git_commit' AND session_id = ?1",
            )?;
            let prefixes: Vec<Option<String>> = stmt
                .query_map(params![session_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for prefix in prefixes.into_iter().flatten() {
                match classify_candidate(&prefix) {
                    CandidateClass::Exact | CandidateClass::Prefix => {}
                    _ => continue,
                }
                // An ambiguous prefix widens the filter rather than narrowing
                // it to a guess: every commit it could mean is included, which
                // is the honest reading of "commits this session may have made".
                out.extend(Self::commits_matching_prefix(&conn, repository, &prefix)?);
            }
        }

        let mut out: Vec<String> = out.into_iter().collect();
        out.sort();
        Ok(out)
    }

    fn commits_without_links(
        &self,
        repository: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>> {
        let conn = self.lock();
        if !Self::table_exists(&conn, "commit_links")? {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT c.sha FROM history_commits c
              WHERE c.repository = ?1
                AND NOT EXISTS (SELECT 1 FROM commit_links l WHERE l.commit_hash = c.sha)
              ORDER BY c.committed_at DESC, c.sha
              LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![repository, limit as i64, offset as i64], |row| {
                row.get(0)
            })?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
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

    fn list_pending_embedding_commits(&self, limit: usize) -> Result<Vec<HistoryCommit>> {
        let conn = self.lock();
        // Oldest first: a backfill that is interrupted repeatedly still makes
        // monotonic progress through history instead of re-attempting the same
        // newest slice every tick.
        let mut stmt = conn.prepare(
            "SELECT * FROM history_commits
              WHERE pending_embedding = 1
              ORDER BY committed_at ASC, sha ASC
              LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], Self::commit_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn list_pending_embedding_docs(&self, limit: usize) -> Result<Vec<HistoryDoc>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT * FROM history_docs
              WHERE pending_embedding = 1
              ORDER BY COALESCE(updated_at, created_at, ''), id
              LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], Self::doc_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn mark_commit_embedded(&self, sha: &str) -> Result<()> {
        self.clear_commit_pending(sha)
    }

    fn skip_commit_embedding(&self, sha: &str) -> Result<()> {
        self.clear_commit_pending(sha)
    }

    fn mark_doc_embedded(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE history_docs SET pending_embedding = 0, embedding_error = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    fn quarantine_commit_embedding(&self, sha: &str, error: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE history_commits SET pending_embedding = 0, embedding_error = ?2 WHERE sha = ?1",
            params![sha, error],
        )?;
        Ok(())
    }

    fn quarantine_doc_embedding(&self, id: &str, error: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE history_docs SET pending_embedding = 0, embedding_error = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    fn count_quarantined_embedding(&self) -> Result<(i64, i64)> {
        let conn = self.lock();
        let commits: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_commits WHERE embedding_error IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let docs: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_docs WHERE embedding_error IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        Ok((commits, docs))
    }

    fn last_quarantined_embedding_error(&self) -> Result<Option<String>> {
        let conn = self.lock();
        let error: Option<String> = conn
            .query_row(
                "SELECT embedding_error FROM (
                     SELECT embedding_error, committed_at AS at FROM history_commits
                         WHERE embedding_error IS NOT NULL
                     UNION ALL
                     SELECT embedding_error, COALESCE(updated_at, created_at, '') AS at
                         FROM history_docs WHERE embedding_error IS NOT NULL
                 ) ORDER BY at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(error)
    }

    fn requeue_quarantined_embeddings(&self) -> Result<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let commits = tx.execute(
            "UPDATE history_commits SET pending_embedding = 1, embedding_error = NULL
             WHERE embedding_error IS NOT NULL",
            [],
        )?;
        let docs = tx.execute(
            "UPDATE history_docs SET pending_embedding = 1, embedding_error = NULL
             WHERE embedding_error IS NOT NULL",
            [],
        )?;
        tx.commit()?;
        Ok(commits + docs)
    }

    fn count_pending_embedding(&self) -> Result<(i64, i64)> {
        let conn = self.lock();
        let commits: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_commits WHERE pending_embedding = 1",
            [],
            |row| row.get(0),
        )?;
        let docs: i64 = conn.query_row(
            "SELECT COUNT(*) FROM history_docs WHERE pending_embedding = 1",
            [],
            |row| row.get(0),
        )?;
        Ok((commits, docs))
    }

    fn mark_all_pending_embedding(&self) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        // A model change invalidates the refusals too: a different model has a
        // different input cap, so a unit this one refused may embed cleanly.
        tx.execute(
            "UPDATE history_commits SET pending_embedding = 1, embedding_error = NULL",
            [],
        )?;
        tx.execute(
            "UPDATE history_docs SET pending_embedding = 1, embedding_error = NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
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

    fn record_epoch(&self, epoch: &HistoryEpoch) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        // If the running executable was deleted/replaced, `binary_path` now
        // names the replacement file and its mtime says nothing about the
        // process being recorded. Never persist that mtime as proof.
        let trusted_binary_mtime = epoch.trusted_binary_mtime();
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
                            binary_mtime = CASE
                                WHEN exe_deleted != 0 OR ?6 != 0 THEN NULL
                                ELSE COALESCE(?3, binary_mtime)
                            END,
                            version = COALESCE(?4, version),
                            ended_at = MAX(COALESCE(?5, ''), COALESCE(ended_at, '')),
                            exe_deleted = MAX(exe_deleted, ?6)
                      WHERE id = ?1",
                    params![
                        id,
                        epoch.binary_path,
                        trusted_binary_mtime,
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
                        trusted_binary_mtime,
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
            "SELECT id, epoch_kind, binary_path,
                    CASE WHEN exe_deleted != 0 THEN NULL ELSE binary_mtime END,
                    version, started_at,
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
    use crate::history_provenance::LINK_METHOD_INDEXER_WORKER_EVENT;
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
            // Test commits have not passed through M3's mapper. Keep this
            // fixture intentionally retryable, as a freshly indexed commit is.
            symbol_mapping: "pending".into(),
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

    /// The M7 queue: what comes out, what clearing it does, and the fact that
    /// it is store-wide. A per-repository list would strand every row belonging
    /// to a repository the drain did not think to name.
    #[test]
    fn the_embedding_queue_lists_marks_and_counts_across_repositories() {
        let (_t, store) = store();
        let mut elsewhere = commit(&"e".repeat(40));
        elsewhere.repository = "/elsewhere".into();
        store
            .commit_batch(
                "/repo",
                &[commit(&"a".repeat(40))],
                &[],
                &"a".repeat(40),
                true,
            )
            .unwrap();
        store
            .commit_batch("/elsewhere", &[elsewhere], &[], &"e".repeat(40), true)
            .unwrap();
        store
            .upsert_docs(
                "/repo",
                SOURCE_GITHUB,
                &[doc(
                    "gh:issue:1",
                    DOC_KIND_ISSUE,
                    "t",
                    "b",
                    "2026-08-02T00:00:00Z",
                )],
                None,
                true,
            )
            .unwrap();

        assert_eq!(store.count_pending_embedding().unwrap(), (2, 1));
        assert_eq!(store.list_pending_embedding_commits(10).unwrap().len(), 2);
        assert_eq!(store.list_pending_embedding_docs(10).unwrap().len(), 1);
        // The limit is honoured, or a backfill would pull the whole corpus into
        // memory on the first tick.
        assert_eq!(store.list_pending_embedding_commits(1).unwrap().len(), 1);

        store.mark_commit_embedded(&"a".repeat(40)).unwrap();
        // A skipped merge leaves the queue exactly as an embedded one does —
        // it is excluded from having a vector, not awaiting one.
        store.skip_commit_embedding(&"e".repeat(40)).unwrap();
        store.mark_doc_embedded("gh:issue:1").unwrap();
        assert_eq!(store.count_pending_embedding().unwrap(), (0, 0));
        assert!(store.list_pending_embedding_commits(10).unwrap().is_empty());

        // A model change re-arms everything, not just what happened to be
        // pending: vectors from two models are not comparable.
        store.mark_all_pending_embedding().unwrap();
        assert_eq!(store.count_pending_embedding().unwrap(), (2, 1));
    }

    /// Re-indexing a commit must not re-enqueue it. Commit prose is immutable,
    /// so a re-walk (branch switch, watermark reset) that re-armed the queue
    /// would bill the whole history again for identical vectors.
    #[test]
    fn reindexing_a_commit_does_not_rearm_its_embedding() {
        let (_t, store) = store();
        let c = commit(&"a".repeat(40));
        store
            .commit_batch("/repo", &[c.clone()], &[], &c.sha, true)
            .unwrap();
        store.mark_commit_embedded(&c.sha).unwrap();

        store
            .commit_batch("/repo", &[c.clone()], &[], &c.sha, true)
            .unwrap();
        assert_eq!(store.count_pending_embedding().unwrap().0, 0);
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
        // M5 (cas-519f): the anchor edge is readable but `events` and
        // `commit_links` are not, so the measurement is PARTIAL and says so.
        // Reporting it as complete would let a store that can only see one of
        // three edges publish a number that reads as the whole picture.
        let reason = cov.unmeasurable_reason.expect("partial measurement declared");
        assert!(reason.contains("events"), "{reason}");
        assert!(reason.contains("commit_links"), "{reason}");
        assert_eq!(
            cov.by_method,
            vec![(LINK_METHOD_FACTORY_ANCHOR.to_string(), 1)]
        );
    }

    /// GH #700 / cas-ba01: the anchor edge used to be a correlated
    /// `EXISTS (SELECT 1 FROM tasks WHERE json_extract(...) = c.sha)`, which
    /// evaluates `json_extract` once per (commit, task) pair. On the reporting
    /// store — 8,509 commits and 4,465 tasks — that one query measured 77.6s,
    /// and `cas doctor` spent 121s of its 126s inside this call.
    ///
    /// The budget is 1s for 4M pairs. The rewritten shape does this in
    /// milliseconds, and the old one is measured below the assertion, so the
    /// margin is against machine speed, not against the defect.
    #[test]
    fn provenance_coverage_stays_linear_in_tasks_not_commits_times_tasks() {
        let (_t, store) = store();
        let commits: Vec<HistoryCommit> = (0..2_000)
            .map(|i| commit(&format!("{i:040x}")))
            .collect();
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE tasks (id TEXT PRIMARY KEY, deliverables TEXT NOT NULL DEFAULT '{}');",
            )
            .unwrap();
            conn.execute_batch("BEGIN").unwrap();
            // Real `deliverables` payloads are kilobytes of receipts, and the
            // cost of the old shape was one JSON parse of that payload per
            // (commit, task) pair. A synthetic store with two-field payloads
            // does not reproduce the defect.
            let filler = "x".repeat(2_000);
            for i in 0..2_000 {
                conn.execute(
                    "INSERT INTO tasks (id, deliverables)
                     VALUES (?1, json_object('factory_branch_anchor', ?2, 'notes', ?3))",
                    params![format!("cas-{i}"), format!("{i:040x}"), filler],
                )
                .unwrap();
            }
            conn.execute_batch("COMMIT").unwrap();
        }
        let tip = commits.last().unwrap().sha.clone();
        store.commit_batch("/repo", &commits, &[], &tip, true).unwrap();

        let started = std::time::Instant::now();
        let cov = store.provenance_coverage("/repo").unwrap();
        let elapsed = started.elapsed();

        assert_eq!(cov.total_commits, 2_000);
        assert_eq!(
            cov.high_confidence_linked,
            Some(2_000),
            "every commit has an anchoring task"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(1_000),
            "provenance coverage took {elapsed:?} for 2,000 commits x 2,000 tasks — the anchor \
             edge is quadratic again"
        );
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

    #[test]
    fn stale_executable_identity_clears_replacement_mtime_and_is_sticky() {
        let (_t, store) = store();
        let started = "2026-08-07T21:02:26Z";
        let mut stale = epoch(started, 42);
        stale.binary_mtime = Some("2026-08-08T00:00:00Z".into());
        stale.exe_deleted = true;
        store.record_epoch(&stale).unwrap();

        let recorded = store.list_epochs(None, 10).unwrap().remove(0);
        assert!(recorded.exe_deleted);
        assert!(
            recorded.binary_mtime.is_none(),
            "the replacement file's fresh mtime must not survive storage"
        );
        let json = serde_json::to_value(&recorded).unwrap();
        assert_eq!(json["binary_mtime"], serde_json::Value::Null);
        assert_eq!(json["exe_deleted"], true);

        // Once an executable is unlinked, a later refresh cannot make that
        // same process identity current again or restore replacement metadata.
        let mut refresh = epoch(started, 42);
        refresh.binary_mtime = Some("2026-08-09T00:00:00Z".into());
        store.record_epoch(&refresh).unwrap();
        let refreshed = store.list_epochs(None, 10).unwrap().remove(0);
        assert!(refreshed.exe_deleted);
        assert!(refreshed.binary_mtime.is_none());

        let conn = store.lock();
        let raw: (Option<String>, i64) = conn
            .query_row(
                "SELECT binary_mtime, exe_deleted FROM history_epochs WHERE pid = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(raw, (None, 1));
    }

    #[test]
    fn stale_mtime_from_an_older_database_is_hidden_on_read() {
        let (_t, store) = store();
        let conn = store.lock();
        conn.execute(
            "INSERT INTO history_epochs (
                 epoch_kind, binary_path, binary_mtime, version,
                 started_at, ended_at, pid, exe_deleted, recorded_at
             ) VALUES (?1, '/usr/local/bin/cas', ?2, '2.49.0', ?3, ?3, 42, 1, ?3)",
            params![
                EPOCH_KIND_DAEMON_START,
                "2026-08-08T00:00:00Z",
                "2026-08-07T21:02:26Z"
            ],
        )
        .unwrap();
        drop(conn);

        let epoch = store.list_epochs(None, 10).unwrap().remove(0);
        assert!(epoch.exe_deleted);
        assert!(
            epoch.binary_mtime.is_none(),
            "legacy replacement metadata must not escape the storage boundary"
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
        assert!(
            epochs.iter().all(|epoch| !epoch.exe_deleted),
            "backfill must report identity as unknown, not invent a stale-executable observation"
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

    // ===================================================================
    // Provenance resolution (EPIC cas-6212 / cas-519f, spec §5.2)
    // ===================================================================

    /// The `events` / `tasks` / `commit_links` tables the resolver reads from
    /// other subsystems. Created explicitly per test so each one states exactly
    /// which edges it is exercising — a fixture that always creates all three
    /// would hide which edge produced a result.
    fn create_events_table(store: &SqliteHistoryStore) {
        store
            .lock()
            .execute_batch(
                "CREATE TABLE events (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     event_type TEXT NOT NULL,
                     entity_type TEXT NOT NULL,
                     entity_id TEXT NOT NULL,
                     summary TEXT NOT NULL,
                     metadata TEXT,
                     created_at TEXT NOT NULL,
                     session_id TEXT
                 );",
            )
            .unwrap();
    }

    fn emit_worker_git_commit(store: &SqliteHistoryStore, head_sha: &str, session: &str) {
        store
            .lock()
            .execute(
                "INSERT INTO events (event_type, entity_type, entity_id, summary, metadata, created_at, session_id)
                 VALUES ('worker_git_commit', 'worker', 'worker-1', 'final git state', ?1, '2026-08-08T01:00:00Z', ?2)",
                params![
                    format!(r#"{{"branch":"factory/w","head_sha":"{head_sha}"}}"#),
                    session
                ],
            )
            .unwrap();
    }

    fn index(store: &SqliteHistoryStore, shas: &[&str]) {
        let commits: Vec<HistoryCommit> = shas.iter().map(|s| commit(s)).collect();
        let last = shas.last().unwrap();
        store
            .commit_batch("/repo", &commits, &[], last, true)
            .unwrap();
    }

    /// §5.2 consequence 1, end to end: the join must use the event's OWN width.
    /// A 7-char row and an 8-char row describe the same commit and both must
    /// resolve — `sha[0..8]` would drop the 594 seven-char rows the live corpus
    /// actually contains.
    #[test]
    fn prefix_matching_resolves_seven_eight_and_forty_char_widths() {
        let (_t, store) = store();
        let sha = "1234567abcdef0123456789abcdef0123456789a";
        index(&store, &[sha]);
        create_events_table(&store);
        emit_worker_git_commit(&store, &sha[..7], "session-7");
        emit_worker_git_commit(&store, &sha[..8], "session-8");
        emit_worker_git_commit(&store, sha, "session-40");

        let resolved = store
            .resolve_provenance("/repo", &[sha.to_string()])
            .unwrap();
        let p = resolved.get(sha).expect("the requested sha is always present");

        let sessions: Vec<&str> = p
            .links
            .iter()
            .filter_map(|l| l.session_id.as_deref())
            .collect();
        assert!(sessions.contains(&"session-7"), "7-char width dropped: {p:?}");
        assert!(sessions.contains(&"session-8"), "8-char width dropped: {p:?}");
        assert!(sessions.contains(&"session-40"), "40-char width dropped: {p:?}");

        // The full-width row is an EXACT match and must be labelled as one, not
        // run through the prefix path with its collision guard.
        let exact = p
            .links
            .iter()
            .find(|l| l.session_id.as_deref() == Some("session-40"))
            .unwrap();
        assert_eq!(exact.link_method, LINK_METHOD_WORKER_EVENT_EXACT);
        assert_eq!(exact.confidence, LinkConfidence::High);
        assert!(!exact.ambiguous);

        let seven = p
            .links
            .iter()
            .find(|l| l.session_id.as_deref() == Some("session-7"))
            .unwrap();
        assert_eq!(seven.link_method, LINK_METHOD_WORKER_EVENT_PREFIX);
        assert_eq!(seven.matched_prefix.as_deref(), Some(&sha[..7]));
    }

    /// The ambiguity test §5.2 says must exist, at the width where the
    /// collision probability is ~1.1% rather than theoretical: two indexed
    /// commits sharing a 7-char prefix. BOTH candidates are returned and the
    /// edge is downgraded — the resolver never picks a winner.
    #[test]
    fn an_ambiguous_seven_char_prefix_returns_every_candidate_and_never_guesses() {
        let (_t, store) = store();
        let a = "7777777aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "7777777bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        index(&store, &[a, b]);
        create_events_table(&store);
        emit_worker_git_commit(&store, "7777777", "session-ambiguous");

        let resolved = store
            .resolve_provenance("/repo", &[a.to_string(), b.to_string()])
            .unwrap();

        for sha in [a, b] {
            let p = resolved.get(sha).unwrap();
            let link = p
                .links
                .iter()
                .find(|l| l.link_method == LINK_METHOD_WORKER_EVENT_PREFIX)
                .unwrap_or_else(|| panic!("{sha} lost its prefix edge: {p:?}"));
            assert!(link.ambiguous, "a colliding prefix must be flagged: {link:?}");
            assert_eq!(
                link.confidence,
                LinkConfidence::Low,
                "an ambiguous edge must not read as good as an exact one"
            );
            let mut candidates = link.ambiguous_candidates.clone();
            candidates.sort();
            assert_eq!(
                candidates,
                vec![a.to_string(), b.to_string()],
                "every commit the prefix could mean must be returned"
            );
        }
    }

    /// An 8-char prefix that happens to be unique in THIS index is not
    /// ambiguous, even though 8-char prefixes can collide in general. Ambiguity
    /// is measured against the corpus, never inferred from width.
    #[test]
    fn ambiguity_is_measured_against_the_index_not_guessed_from_width() {
        let (_t, store) = store();
        let a = "8888888aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "8888888baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        index(&store, &[a, b]);
        create_events_table(&store);
        // 8 chars distinguishes them; 7 would not.
        emit_worker_git_commit(&store, &a[..8], "session-unique");

        let resolved = store.resolve_provenance("/repo", &[a.to_string(), b.to_string()]).unwrap();
        let link = resolved.get(a).unwrap().links.first().cloned().unwrap();
        assert!(!link.ambiguous);
        assert_eq!(link.confidence, LinkConfidence::High);
        // ...and the sibling commit is untouched by an edge that is not its own.
        assert!(resolved.get(b).unwrap().links.is_empty());
    }

    /// §5.2 consequence 2: the `'?'` sentinel is a declared degradation, not a
    /// SHA. It must never resolve, and it must be COUNTED so the 46 rows
    /// carrying it stay visible instead of looking like a coverage gap.
    #[test]
    fn the_stub_and_the_empty_rows_are_excluded_and_counted() {
        let (_t, store) = store();
        let sha = "9999999aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        index(&store, &[sha]);
        create_events_table(&store);
        emit_worker_git_commit(&store, "?", "session-stub");
        emit_worker_git_commit(&store, "999", "session-tooshort");
        store
            .lock()
            .execute(
                "INSERT INTO events (event_type, entity_type, entity_id, summary, metadata, created_at, session_id)
                 VALUES ('worker_git_commit', 'worker', 'w', 's', NULL, '2026-08-08T01:00:00Z', 'session-null')",
                [],
            )
            .unwrap();

        let resolved = store.resolve_provenance("/repo", &[sha.to_string()]).unwrap();
        let p = resolved.get(sha).unwrap();
        assert!(p.links.is_empty(), "no usable edge existed: {p:?}");
        assert!(
            p.reason.as_deref().unwrap_or_default().contains("no populated edge"),
            "an unresolved commit must say why: {p:?}"
        );

        let cov = store.provenance_coverage("/repo").unwrap();
        let edge = cov
            .edges
            .iter()
            .find(|e| e.edge == LINK_METHOD_WORKER_EVENT_PREFIX)
            .expect("the event edge is reported even when it contributes nothing");
        assert_eq!(edge.usable_rows, 0);
        assert_eq!(edge.excluded_stub, 1, "the '?' stub must be counted, not dropped");
        assert_eq!(edge.excluded_absent, 1, "the NULL-metadata class must be counted");
        assert_eq!(edge.excluded_unusable, 1, "the too-short class must be counted");
    }

    /// §6.4 Q3: a commit with no edge is part of the answer, carrying its
    /// reason. Dropping it would turn "we cannot attribute this" into "this
    /// commit does not exist".
    #[test]
    fn a_commit_with_no_edge_is_returned_with_a_reason_never_omitted() {
        let (_t, store) = store();
        let a = "aaaaaaa1111111111111111111111111111111aa";
        index(&store, &[a]);

        let resolved = store.resolve_provenance("/repo", &[a.to_string()]).unwrap();
        assert_eq!(resolved.len(), 1, "every requested sha must be present");
        let p = &resolved[a];
        assert!(p.links.is_empty());
        assert!(p.reason.is_some());
        assert!(p.best().is_none());
    }

    /// The exact anchor edge names a TASK, and does so at high confidence with
    /// no prefix arithmetic anywhere near it.
    #[test]
    fn the_factory_anchor_edge_resolves_a_task_at_high_confidence() {
        let (_t, store) = store();
        let sha = "abcdef01234567890123456789012345678901ab";
        index(&store, &[sha]);
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE tasks (
                     id TEXT PRIMARY KEY, title TEXT, notes TEXT, updated_at TEXT,
                     assignee TEXT, deliverables TEXT NOT NULL DEFAULT '{}');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, title, updated_at, assignee, deliverables)
                 VALUES ('cas-519f', 'M5 provenance', '2026-08-08T00:00:00Z', 'worker-1',
                         json_object('factory_branch_anchor', ?1))",
                params![sha],
            )
            .unwrap();
        }

        let resolved = store.resolve_provenance("/repo", &[sha.to_string()]).unwrap();
        let link = resolved[sha].best().cloned().unwrap();
        assert_eq!(link.link_method, LINK_METHOD_FACTORY_ANCHOR);
        assert_eq!(link.confidence, LinkConfidence::High);
        assert_eq!(link.task_id.as_deref(), Some("cas-519f"));
        assert_eq!(link.task_title.as_deref(), Some("M5 provenance"));
        assert!(link.matched_prefix.is_none(), "an exact edge has no prefix to report");

        // And the same edge answers the task_id filter.
        assert_eq!(
            store.shas_for_task("/repo", "cas-519f").unwrap(),
            vec![sha.to_string()]
        );
    }

    /// A close-note mention is evidence, but it is a substring of free text, so
    /// it can never outrank an exact edge — and it must not duplicate one.
    #[test]
    fn the_note_edge_is_medium_and_never_duplicates_the_anchor() {
        let (_t, store) = store();
        let anchored = "beef000000000000000000000000000000000001";
        let mentioned = "beef000000000000000000000000000000000002";
        index(&store, &[anchored, mentioned]);
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE tasks (
                     id TEXT PRIMARY KEY, title TEXT, notes TEXT, updated_at TEXT,
                     assignee TEXT, deliverables TEXT NOT NULL DEFAULT '{}');",
            )
            .unwrap();
            // One task both anchors `anchored` AND mentions both SHAs in prose.
            conn.execute(
                "INSERT INTO tasks (id, title, notes, updated_at, assignee, deliverables)
                 VALUES ('cas-1', 't', ?1, '2026-08-08T00:00:00Z', 'w',
                         json_object('factory_branch_anchor', ?2))",
                params![
                    format!("resolved to full commit {anchored}; see also {mentioned}"),
                    anchored
                ],
            )
            .unwrap();
        }

        let resolved = store
            .resolve_provenance("/repo", &[anchored.to_string(), mentioned.to_string()])
            .unwrap();

        // The anchored commit gets ONE link, not an anchor plus a redundant
        // text hit on the same task: fake corroboration is still fake.
        let a = &resolved[anchored];
        assert_eq!(a.links.len(), 1, "{a:?}");
        assert_eq!(a.links[0].link_method, LINK_METHOD_FACTORY_ANCHOR);

        let m = &resolved[mentioned];
        assert_eq!(m.links.len(), 1, "{m:?}");
        assert_eq!(m.links[0].link_method, LINK_METHOD_TASK_NOTE);
        assert_eq!(m.links[0].confidence, LinkConfidence::Medium);
    }

    /// The `shas` filter must narrow in SQL. `Some(empty)` means "this filter
    /// matched nothing", and the one thing it must never do is widen back out
    /// to every commit.
    #[test]
    fn an_empty_sha_filter_matches_nothing_rather_than_everything() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        index(&store, &[&a, &b]);

        let none = store
            .search_commits(&HistoryQuery {
                repository: "/repo".into(),
                shas: Some(Vec::new()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(none.is_empty(), "an empty allowlist must match nothing");

        let one = store
            .search_commits(&HistoryQuery {
                repository: "/repo".into(),
                shas: Some(vec![a.clone()]),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].commit.sha, a);

        // And it composes with the other filters rather than replacing them.
        let with_text = store
            .search_commits(&HistoryQuery {
                repository: "/repo".into(),
                text: Some("subject".into()),
                shas: Some(vec![b.clone()]),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(with_text.len(), 1);
        assert_eq!(with_text[0].commit.sha, b);
    }

    /// The session filter widens across an ambiguous prefix instead of
    /// narrowing to a guess: "commits this session may have made" is the honest
    /// reading of ambiguous evidence.
    #[test]
    fn the_session_filter_resolves_through_events_and_keeps_ambiguity_inclusive() {
        let (_t, store) = store();
        let a = "5555555aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "5555555bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        index(&store, &[a, b]);
        create_events_table(&store);
        emit_worker_git_commit(&store, "5555555", "session-x");

        let mut shas = store.shas_for_session("/repo", "session-x").unwrap();
        shas.sort();
        assert_eq!(shas, vec![a.to_string(), b.to_string()]);
        assert!(store.shas_for_session("/repo", "session-none").unwrap().is_empty());
    }

    /// The repair work list is "indexed commits with no link", so a commit that
    /// already has one drops off it — which is what makes the pass idempotent
    /// and safe to run on every daemon tick.
    #[test]
    fn commits_without_links_is_the_repair_work_list() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        index(&store, &[&a, &b]);
        assert!(
            store.commits_without_links("/repo", 10, 0).unwrap().is_empty(),
            "with no commit_links table there is no spine to repair"
        );

        store
            .lock()
            .execute_batch(crate::commit_link_store::COMMIT_LINK_SCHEMA)
            .unwrap();
        let mut pending = store.commits_without_links("/repo", 10, 0).unwrap();
        pending.sort();
        assert_eq!(pending, vec![a.clone(), b.clone()]);
        // The offset is what lets a "repair everything" loop step past commits
        // that will never resolve, instead of re-reading them forever.
        assert_eq!(store.commits_without_links("/repo", 10, 2).unwrap().len(), 0);
        assert_eq!(store.commits_without_links("/repo", 1, 1).unwrap().len(), 1);

        store
            .lock()
            .execute(
                "INSERT INTO commit_links (commit_hash, session_id, agent_id, branch, message,
                     files_changed, prompt_ids, committed_at, author, scope, link_method)
                 VALUES (?1, 's', 'a', 'main', 'm', '[]', '[]', '2026-08-08T00:00:00Z', 'x', 'project', 'hook_observed')",
                params![a],
            )
            .unwrap();
        assert_eq!(store.commits_without_links("/repo", 10, 0).unwrap(), vec![b]);
    }

    /// A `commit_links` row written before M5 has no `link_method`. It must be
    /// read as the observation it was — the hook was the only writer then — and
    /// not as an unknown or a reconstruction.
    #[test]
    fn a_legacy_link_row_reads_as_an_observation() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        index(&store, &[&a]);
        store
            .lock()
            .execute_batch(crate::commit_link_store::COMMIT_LINK_SCHEMA)
            .unwrap();
        store
            .lock()
            .execute(
                "INSERT INTO commit_links (commit_hash, session_id, agent_id, branch, message,
                     files_changed, prompt_ids, committed_at, author, scope)
                 VALUES (?1, 'legacy-session', 'legacy-agent', 'main', 'm', '[]', '[]',
                         '2026-08-08T00:00:00Z', 'x', 'project')",
                params![a],
            )
            .unwrap();

        let resolved = store.resolve_provenance("/repo", &[a.clone()]).unwrap();
        let link = resolved[&a].best().cloned().unwrap();
        assert_eq!(link.link_method, LINK_METHOD_HOOK_OBSERVED);
        assert_eq!(link.confidence, LinkConfidence::High);
        assert_eq!(link.session_id.as_deref(), Some("legacy-session"));
    }

    /// A reconstructed row is reported at MEDIUM even though it lives in the
    /// same table as an observation — the whole point of `link_method`.
    #[test]
    fn a_reconstructed_link_row_never_reports_as_an_observation() {
        let (_t, store) = store();
        let a = "a".repeat(40);
        index(&store, &[&a]);
        store
            .lock()
            .execute_batch(crate::commit_link_store::COMMIT_LINK_SCHEMA)
            .unwrap();
        store
            .lock()
            .execute(
                "INSERT INTO commit_links (commit_hash, session_id, agent_id, branch, message,
                     files_changed, prompt_ids, committed_at, author, scope, link_method)
                 VALUES (?1, 's', 'a', 'main', 'm', '[]', '[]', '2026-08-08T00:00:00Z', 'x',
                         'project', ?2)",
                params![a, LINK_METHOD_INDEXER_WORKER_EVENT],
            )
            .unwrap();

        let resolved = store.resolve_provenance("/repo", &[a.clone()]).unwrap();
        let link = resolved[&a].best().cloned().unwrap();
        assert_eq!(link.link_method, LINK_METHOD_INDEXER_WORKER_EVENT);
        assert_eq!(link.confidence, LinkConfidence::Medium);
    }

    /// Coverage must report both figures split by confidence (spec §10.1), and
    /// the per-method breakdown that makes the split auditable.
    #[test]
    fn coverage_reports_high_and_any_confidence_separately() {
        let (_t, store) = store();
        let anchored = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let evented = "2222222bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let orphan = "3333333ccccccccccccccccccccccccccccccccc";
        index(&store, &[anchored, evented, orphan]);
        create_events_table(&store);
        emit_worker_git_commit(&store, &evented[..8], "session-1");
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT, notes TEXT,
                     updated_at TEXT, assignee TEXT, deliverables TEXT NOT NULL DEFAULT '{}');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, deliverables)
                 VALUES ('cas-1', json_object('factory_branch_anchor', ?1))",
                params![anchored],
            )
            .unwrap();
        }

        let cov = store.provenance_coverage("/repo").unwrap();
        assert_eq!(cov.total_commits, 3);
        // High confidence counts ONLY the exact edge...
        assert_eq!(cov.high_confidence_linked, Some(1));
        // ...while "any edge" adds the prefix one. Publishing only the second
        // number would make a substring-grade corpus look solved.
        assert_eq!(cov.any_confidence_linked, Some(2));
        assert!((cov.coverage_pct.unwrap() - 33.33).abs() < 0.1, "{cov:?}");
        assert!((cov.any_coverage_pct.unwrap() - 66.66).abs() < 0.1, "{cov:?}");
        assert!(
            cov.by_method
                .contains(&(LINK_METHOD_FACTORY_ANCHOR.to_string(), 1))
        );
        assert!(
            cov.by_method
                .contains(&(LINK_METHOD_WORKER_EVENT_PREFIX.to_string(), 1))
        );
    }

    /// GH #695: a unit the provider refuses must leave the pending queue while
    /// keeping the refusal, so the backlog can reach zero, the reason survives
    /// for reporting, and an operator can re-arm it after the cause is fixed.
    #[test]
    fn quarantined_units_leave_pending_keep_their_reason_and_can_be_requeued() {
        let (_t, store) = store();
        let poison = "a".repeat(40);
        let healthy = "b".repeat(40);
        store
            .commit_batch(
                "/repo",
                &[commit(&poison), commit(&healthy)],
                &[],
                &healthy,
                true,
            )
            .unwrap();

        assert_eq!(store.count_pending_embedding().unwrap(), (2, 0));

        store
            .quarantine_commit_embedding(&poison, "provider refused: input over token cap")
            .unwrap();

        assert_eq!(
            store.count_pending_embedding().unwrap(),
            (1, 0),
            "a refused unit must stop blocking the queue"
        );
        assert_eq!(store.count_quarantined_embedding().unwrap(), (1, 0));
        assert_eq!(
            store.last_quarantined_embedding_error().unwrap().as_deref(),
            Some("provider refused: input over token cap")
        );
        let still_pending = store.list_pending_embedding_commits(10).unwrap();
        assert_eq!(still_pending.len(), 1);
        assert_eq!(still_pending[0].sha, healthy, "the healthy unit still drains");

        // Re-arming is the operator escape hatch once the cause is repaired.
        assert_eq!(store.requeue_quarantined_embeddings().unwrap(), 1);
        assert_eq!(store.count_quarantined_embedding().unwrap(), (0, 0));
        assert_eq!(store.count_pending_embedding().unwrap(), (2, 0));
        assert_eq!(store.last_quarantined_embedding_error().unwrap(), None);

        // A later success must clear the refusal rather than leave it counting.
        store
            .quarantine_commit_embedding(&poison, "provider refused again")
            .unwrap();
        store.mark_commit_embedded(&poison).unwrap();
        assert_eq!(store.count_quarantined_embedding().unwrap(), (0, 0));
    }

    /// A store created before the column existed gains it on open, keeping its
    /// rows — the upgrade path every reporting host is on.
    #[test]
    fn embedding_error_column_is_added_to_a_legacy_history_store() {
        let temp = TempDir::new().unwrap();
        {
            let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE history_commits (
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
                 INSERT INTO history_commits
                     (sha, short_sha, committed_at, subject, repository, indexed_at)
                 VALUES ('legacy', 'legacy', '2026-01-01T00:00:00Z', 's', '/repo', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }

        let store = SqliteHistoryStore::open(temp.path()).unwrap();
        store.init().unwrap();

        assert_eq!(store.count_pending_embedding().unwrap().0, 1);
        assert_eq!(store.count_quarantined_embedding().unwrap(), (0, 0));
        store
            .quarantine_commit_embedding("legacy", "refused")
            .unwrap();
        assert_eq!(store.count_quarantined_embedding().unwrap(), (1, 0));
    }

}
