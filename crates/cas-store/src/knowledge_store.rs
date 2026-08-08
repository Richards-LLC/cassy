//! Project-knowledge store: LLM-distilled prose pages with source provenance.
//!
//! The store is deliberately split-brain:
//!
//! - **Bodies** are markdown files under `<cas_dir>/knowledge/<rel_path>`. They
//!   stay greppable, diffable and hand-editable. No SQLite column ever holds
//!   body prose (the contentless FTS index below stores tokenized terms only).
//! - **Index + ledger** live in `cas.db`: [`knowledge_pages`] (one row per page,
//!   holding only the metadata + a short snippet), `knowledge_sources` (a
//!   content-hash ledger of every source file the distiller has seen) and
//!   `knowledge_pages_fts` (a *contentless* FTS5 index over title + snippet +
//!   body, so the body is searchable without being stored twice).
//!
//! # Crash-consistency contract
//!
//! [`KnowledgeStore::commit_ingest`] is the only mutation path used by the
//! distillation pipeline, and it applies **page rows, FTS index rows, source
//! ledger status and tombstone removal in ONE SQLite transaction**. A crash can
//! therefore never leave the ledger claiming `ingested` while the index is
//! missing the page (or vice versa).
//!
//! Body files are two-phase too (see `BodyTransaction`): each body is staged as
//! a temp file, published into place only once its row write has succeeded, and
//! rolled back — new files removed, overwritten files restored from backup — if
//! the transaction aborts. So a failed pass leaves `.cas/knowledge/` byte-for-
//! byte as it was, and a committed row always has its body on disk.
//!
//! A hard process kill between the last publish and `COMMIT` can still strand an
//! orphan markdown file with no row. That direction is the safe one (stale file,
//! never a dangling row) and the next pass over the same page overwrites it.
//!
//! [`knowledge_pages`]: KNOWLEDGE_SCHEMA

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::error::StoreError;
use crate::shared_db::{self, ImmediateTx};

/// Directory (relative to `.cas/`) holding markdown page bodies.
pub const KNOWLEDGE_DIR_NAME: &str = "knowledge";

/// Canonical DDL for the knowledge subsystem, in `execute_batch` form.
///
/// Note the FTS5 table is **contentless** (`content=''`): it stores the
/// inverted index for the body but not the body text itself, which keeps the
/// "bodies never enter SQLite" invariant while still making bodies searchable.
/// `contentless_delete=1` (SQLite 3.43+) is what lets a rebuild delete and
/// re-insert a page's index row inside the ingest transaction.
pub const KNOWLEDGE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_pages (
    row_id INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    page_type TEXT NOT NULL,
    title TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    snippet TEXT NOT NULL DEFAULT '',
    locked INTEGER NOT NULL DEFAULT 0,
    sources_json TEXT NOT NULL DEFAULT '[]',
    origin TEXT NOT NULL DEFAULT 'local' CHECK (origin IN ('local', 'cloud_pull')),
    origin_project_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    pending_embedding INTEGER NOT NULL DEFAULT 1
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_knowledge_pages_rel_path
    ON knowledge_pages(rel_path);
CREATE INDEX IF NOT EXISTS idx_knowledge_pages_type
    ON knowledge_pages(page_type);
CREATE INDEX IF NOT EXISTS idx_knowledge_pages_pending_embedding
    ON knowledge_pages(updated_at) WHERE pending_embedding = 1;

-- Durable deletion ledger for cloud knowledge-page tombstones.  A tombstone
-- survives the local row it removed, so an old page record cannot revive it
-- on a later pull. `locally_authored` decides whether this client must emit
-- it; pulled tombstones are protection state only and are never echoed back.
CREATE TABLE IF NOT EXISTS knowledge_page_tombstones (
    id TEXT PRIMARY KEY,
    deleted_at TEXT NOT NULL,
    locally_authored INTEGER NOT NULL DEFAULT 0,
    pushed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_knowledge_page_tombstones_pending_push
    ON knowledge_page_tombstones(deleted_at)
    WHERE locally_authored = 1 AND pushed_at IS NULL;

CREATE TABLE IF NOT EXISTS knowledge_sources (
    file_path TEXT PRIMARY KEY,
    blake3 TEXT NOT NULL,
    size INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('uploaded', 'ingested', 'failed')),
    ingest_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_pages_fts USING fts5(
    title,
    snippet,
    body,
    content='',
    contentless_delete=1
);
"#;

/// Statement-level form of [`KNOWLEDGE_SCHEMA`] for the numbered migration
/// runner, which calls `Connection::execute` once per item.
///
/// Keep in lockstep with [`KNOWLEDGE_SCHEMA`]; the migration test compares the
/// resulting table/index shapes.
pub const KNOWLEDGE_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS knowledge_pages (
        row_id INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        page_type TEXT NOT NULL,
        title TEXT NOT NULL,
        rel_path TEXT NOT NULL,
        snippet TEXT NOT NULL DEFAULT '',
        locked INTEGER NOT NULL DEFAULT 0,
        sources_json TEXT NOT NULL DEFAULT '[]',
        origin TEXT NOT NULL DEFAULT 'local' CHECK (origin IN ('local', 'cloud_pull')),
        origin_project_id TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        pending_embedding INTEGER NOT NULL DEFAULT 1
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_knowledge_pages_rel_path
        ON knowledge_pages(rel_path)",
    "CREATE INDEX IF NOT EXISTS idx_knowledge_pages_type
        ON knowledge_pages(page_type)",
    "CREATE INDEX IF NOT EXISTS idx_knowledge_pages_pending_embedding
        ON knowledge_pages(updated_at) WHERE pending_embedding = 1",
    "CREATE TABLE IF NOT EXISTS knowledge_page_tombstones (
        id TEXT PRIMARY KEY,
        deleted_at TEXT NOT NULL,
        locally_authored INTEGER NOT NULL DEFAULT 0,
        pushed_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_knowledge_page_tombstones_pending_push
        ON knowledge_page_tombstones(deleted_at)
        WHERE locally_authored = 1 AND pushed_at IS NULL",
    "CREATE TABLE IF NOT EXISTS knowledge_sources (
        file_path TEXT PRIMARY KEY,
        blake3 TEXT NOT NULL,
        size INTEGER NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('uploaded', 'ingested', 'failed')),
        ingest_error TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_pages_fts USING fts5(
        title,
        snippet,
        body,
        content='',
        contentless_delete=1
    )",
];

/// Upgrade statements for stores created before page provenance was durable.
///
/// SQLite applies the `local` default to every existing row while adding the
/// column. That is the only honest backfill: those pages predate cloud-pull
/// attribution, and global knowledge stores have no project id to synthesize.
pub const KNOWLEDGE_PAGE_ATTRIBUTION_STATEMENTS: &[&str] = &[
    "ALTER TABLE knowledge_pages ADD COLUMN origin TEXT NOT NULL DEFAULT 'local'
        CHECK (origin IN ('local', 'cloud_pull'))",
    "ALTER TABLE knowledge_pages ADD COLUMN origin_project_id TEXT",
];

/// Durable tombstone ledger for knowledge-page cloud deletes.
///
/// Kept separate from [`KNOWLEDGE_SCHEMA_STATEMENTS`] because installs that
/// already ran m219 need a numbered migration to gain this new table.
pub const KNOWLEDGE_PAGE_TOMBSTONE_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS knowledge_page_tombstones (
        id TEXT PRIMARY KEY,
        deleted_at TEXT NOT NULL,
        locally_authored INTEGER NOT NULL DEFAULT 0,
        pushed_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_knowledge_page_tombstones_pending_push
        ON knowledge_page_tombstones(deleted_at)
        WHERE locally_authored = 1 AND pushed_at IS NULL",
];

// ── Hashing ─────────────────────────────────────────────────────────────

/// BLAKE3 content hash, lowercase hex — the canonical source-identity function
/// for the ledger. Every producer must use this so ledger comparisons agree.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Hash a source file on disk, returning `(blake3_hex, size_in_bytes)`.
pub fn hash_source_file(path: &Path) -> Result<(String, u64)> {
    let bytes = std::fs::read(path)?;
    let len = bytes.len() as u64;
    Ok((blake3_hex(&bytes), len))
}

// ── Types ───────────────────────────────────────────────────────────────

/// Lifecycle state of a source file in the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    /// Seen and hashed, distillation not finished yet.
    Uploaded,
    /// Fully distilled into at least one page; skip until the hash changes.
    Ingested,
    /// Distillation failed; retried automatically on the next pass.
    Failed,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uploaded => "uploaded",
            Self::Ingested => "ingested",
            Self::Failed => "failed",
        }
    }
}

impl FromStr for SourceStatus {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "uploaded" => Ok(Self::Uploaded),
            "ingested" => Ok(Self::Ingested),
            "failed" => Ok(Self::Failed),
            other => Err(StoreError::Parse(format!(
                "invalid knowledge source status '{other}'; expected uploaded, ingested, or failed"
            ))),
        }
    }
}

/// One row of the source ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeSource {
    pub file_path: String,
    pub blake3: String,
    pub size: u64,
    pub status: SourceStatus,
    pub ingest_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A source file as observed on disk (no ledger state attached).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSource {
    pub file_path: String,
    pub blake3: String,
    pub size: u64,
}

/// Why the classifier wants a source (re-)distilled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestReason {
    /// Not present in the ledger at all.
    New,
    /// Present but the content hash (or size) moved.
    Changed,
    /// Present with the same hash but a non-`ingested` status — auto-retry.
    Retry,
}

/// A source the classifier selected for distillation, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSource {
    pub source: DiskSource,
    pub reason: IngestReason,
}

/// Result of comparing the on-disk source set against the ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceClassification {
    /// Sources that must be (re-)distilled.
    pub to_ingest: Vec<PendingSource>,
    /// Sources already ingested at the exact same content hash.
    pub skipped: Vec<DiskSource>,
    /// Ledger paths with no counterpart on disk — tombstone them.
    pub deleted: Vec<String>,
}

/// Compare the on-disk source set against the ledger. Pure: no I/O, no clock,
/// deterministic ordering (disk order for ingest/skip, ledger order for
/// deletions), so it is unit-testable in isolation.
///
/// Rules:
/// - unknown path → [`IngestReason::New`]
/// - known path, different hash or size → [`IngestReason::Changed`]
/// - known path, same hash, `status != ingested` → [`IngestReason::Retry`]
///   (this is what makes `failed` and half-finished `uploaded` rows self-heal)
/// - known path, same hash, `status == ingested` → skipped
/// - ledger path absent from disk → deleted
///
/// Duplicate disk entries for the same path are collapsed: only the first wins.
pub fn classify_sources(disk: &[DiskSource], ledger: &[KnowledgeSource]) -> SourceClassification {
    let ledger_by_path: HashMap<&str, &KnowledgeSource> = ledger
        .iter()
        .map(|row| (row.file_path.as_str(), row))
        .collect();

    let mut classification = SourceClassification::default();
    let mut seen_on_disk: HashSet<&str> = HashSet::new();

    for entry in disk {
        if !seen_on_disk.insert(entry.file_path.as_str()) {
            continue;
        }

        match ledger_by_path.get(entry.file_path.as_str()) {
            None => classification.to_ingest.push(PendingSource {
                source: entry.clone(),
                reason: IngestReason::New,
            }),
            Some(row) if row.blake3 != entry.blake3 || row.size != entry.size => {
                classification.to_ingest.push(PendingSource {
                    source: entry.clone(),
                    reason: IngestReason::Changed,
                })
            }
            Some(row) if row.status != SourceStatus::Ingested => {
                classification.to_ingest.push(PendingSource {
                    source: entry.clone(),
                    reason: IngestReason::Retry,
                })
            }
            Some(_) => classification.skipped.push(entry.clone()),
        }
    }

    for row in ledger {
        if !seen_on_disk.contains(row.file_path.as_str()) {
            classification.deleted.push(row.file_path.clone());
        }
    }

    classification
}

/// Index row for one distilled knowledge page. The prose body is **not** here —
/// it lives at `<cas_dir>/knowledge/<rel_path>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgePage {
    pub id: String,
    /// Coarse category (`architecture`, `subsystem`, `workflow`, …). Drives the
    /// canonical path, so a re-distilled page merges instead of duplicating.
    pub page_type: String,
    pub title: String,
    /// Path of the markdown body relative to the knowledge dir, e.g.
    /// `architecture/build-system.md`.
    pub rel_path: String,
    /// Short index-injectable summary (one or two sentences).
    pub snippet: String,
    /// User-sovereignty bit: a locked page is never overwritten by distillation.
    pub locked: bool,
    /// Source file paths this page was distilled from (provenance).
    pub sources: Vec<String>,
    /// How this copy entered the local knowledge store.
    pub origin: KnowledgePageOrigin,
    /// Canonical project id asserted by the cloud row that produced this copy.
    /// Local pages deliberately leave this empty: global knowledge stores have
    /// no project identity, so inventing one during migration would be false
    /// provenance.
    pub origin_project_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Whether the page still needs an embedding computed (cloud-gated, T5).
    pub pending_embedding: bool,
}

impl KnowledgePage {
    /// Build a page with a canonical `rel_path` derived from type + title.
    pub fn new(
        id: impl Into<String>,
        page_type: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        let page_type = page_type.into();
        let title = title.into();
        let rel_path = canonical_rel_path(&page_type, &title);
        let now = Utc::now();
        Self {
            id: id.into(),
            page_type,
            title,
            rel_path,
            snippet: String::new(),
            locked: false,
            sources: Vec::new(),
            origin: KnowledgePageOrigin::Local,
            origin_project_id: None,
            created_at: now,
            updated_at: now,
            pending_embedding: true,
        }
    }
}

/// Durable provenance for a knowledge-page row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgePageOrigin {
    /// Created on this machine by distillation, migration, or a manual write.
    Local,
    /// Applied from the cloud knowledge-pull path.
    CloudPull,
}

impl KnowledgePageOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::CloudPull => "cloud_pull",
        }
    }
}

impl FromStr for KnowledgePageOrigin {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "cloud_pull" => Ok(Self::CloudPull),
            other => Err(StoreError::Parse(format!(
                "invalid knowledge page origin '{other}'; expected local or cloud_pull"
            ))),
        }
    }
}

/// A page plus the markdown body to persist alongside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageWrite {
    pub page: KnowledgePage,
    pub body: String,
}

/// The outcome of distilling one source file, destined for the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOutcome {
    pub file_path: String,
    pub blake3: String,
    pub size: u64,
    pub status: SourceStatus,
    pub ingest_error: Option<String>,
}

/// Everything one distillation pass wants to make durable, applied atomically.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestBatch {
    /// Pages to create or merge-update (keyed by canonical `rel_path`).
    pub pages: Vec<PageWrite>,
    /// Per-source ledger transitions for this pass.
    pub sources: Vec<SourceOutcome>,
    /// Ledger paths whose files vanished from disk.
    pub tombstones: Vec<String>,
}

/// What [`KnowledgeStore::commit_ingest`] actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub pages_written: usize,
    /// `rel_path`s (NOT ids) skipped because the existing row is `locked`.
    pub locked_skipped_rel_paths: Vec<String>,
    pub sources_recorded: usize,
    pub sources_tombstoned: usize,
    /// Page IDs cascade-deleted because their last source was tombstoned.
    pub cascade_deleted_page_ids: Vec<String>,
    /// Page IDs refused because a durable local or remote tombstone already
    /// exists. This is the no-resurrection guard for stale page records.
    pub tombstoned_skipped_page_ids: Vec<String>,
}

/// A durable knowledge-page deletion record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgePageTombstone {
    pub id: String,
    pub deleted_at: DateTime<Utc>,
    /// True only when this machine deleted the page and must send the
    /// tombstone to the cloud. Pulled tombstones remain local guard state.
    pub locally_authored: bool,
}

/// Result of applying an incoming tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneApplyOutcome {
    /// The local row (if any) was deleted and the tombstone guard recorded.
    Applied,
    /// A locally locked page was deliberately retained. The tombstone guard
    /// is still recorded so an older page record cannot revive elsewhere.
    LockedPreserved,
}

/// A full-text search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeHit {
    pub page: KnowledgePage,
    /// BM25 score (lower is a better match, as SQLite reports it).
    pub score: f64,
}

// ── Path canonicalization ───────────────────────────────────────────────

/// Lowercase ASCII slug: alphanumerics kept, everything else collapsed to `-`.
pub fn slugify(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    let mut last_dash = true; // suppresses a leading dash
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug.truncate(80);
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Canonical body path for a page: `<type-slug>/<title-slug>.md`.
///
/// This is the merge-not-duplicate rule — distilling the same subject twice
/// resolves to the same path and therefore updates one row instead of adding a
/// near-duplicate.
pub fn canonical_rel_path(page_type: &str, title: &str) -> String {
    let type_slug = match slugify(page_type) {
        s if s.is_empty() => "general".to_string(),
        s => s,
    };
    let title_slug = match slugify(title) {
        s if s.is_empty() => "untitled".to_string(),
        s => s,
    };
    format!("{type_slug}/{title_slug}.md")
}

/// Reject anything that could escape the knowledge directory or collide with a
/// non-markdown file. `rel_path` comes from LLM output, so this is load-bearing.
fn validate_rel_path(rel_path: &str) -> Result<()> {
    if rel_path.trim().is_empty() {
        return Err(StoreError::Parse(
            "knowledge page rel_path must not be empty".to_string(),
        ));
    }
    if !rel_path.ends_with(".md") {
        return Err(StoreError::Parse(format!(
            "knowledge page rel_path must end with .md: {rel_path}"
        )));
    }
    if rel_path.contains('\\') || rel_path.contains('\0') {
        return Err(StoreError::Parse(format!(
            "knowledge page rel_path contains an illegal character: {rel_path}"
        )));
    }
    let path = Path::new(rel_path);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(StoreError::Parse(format!(
                    "knowledge page rel_path must be a relative path without '..': {rel_path}"
                )));
            }
        }
    }
    Ok(())
}

/// Prefilter for "which pages cite this source path".
///
/// `sources_json` holds a JSON array, so the bound parameter must be the
/// JSON-ESCAPED form of the path — matching the raw path misses any path
/// containing `"` or `\` (see [`json_fragment`]). The prefilter is only a
/// prefilter: callers still confirm with an exact element comparison, because
/// `%`/`_` in a path widen the LIKE and one path can be a substring of another.
const PAGES_CITING_SOURCE_SQL: &str = "SELECT row_id, id, rel_path, sources_json, locked
     FROM knowledge_pages
     WHERE sources_json LIKE '%' || ?1 || '%'";

/// The way `value` appears inside a serialized JSON array, without the
/// surrounding quotes — i.e. what to match against `sources_json`.
fn json_fragment(value: &str) -> String {
    let quoted = serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""));
    quoted
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(&quoted)
        .to_string()
}

/// Two-phase commit for page body files, so the filesystem rolls back with the
/// database instead of drifting ahead of it.
///
/// Bodies are staged as temp files, `publish`ed into place only once their row
/// write has succeeded, and — if the object is dropped without [`Self::commit`]
/// — every publish is undone: newly created files are removed and overwritten
/// files are restored from their backup. Combined with `ImmediateTx`'s
/// rollback-on-drop, an aborted pass leaves BOTH the database and
/// `.cas/knowledge/` exactly as they were.
struct BodyTransaction {
    /// (final path, backup of the previous contents if the file already existed)
    published: Vec<(PathBuf, Option<PathBuf>)>,
    staged: Vec<PathBuf>,
    committed: bool,
}

impl BodyTransaction {
    fn new() -> Self {
        Self {
            published: Vec::new(),
            staged: Vec::new(),
            committed: false,
        }
    }

    /// Write `body` to a temp file next to `final_path` and remember it.
    fn stage(&mut self, final_path: &Path, index: usize, body: &str) -> Result<PathBuf> {
        let parent = final_path.parent().ok_or_else(|| {
            StoreError::Parse(format!("knowledge body path has no parent: {final_path:?}"))
        })?;
        let file_name = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("page.md");
        let temp = parent.join(format!(".{file_name}.tmp-{}-{index}", std::process::id()));
        std::fs::write(&temp, body)?;
        self.staged.push(temp.clone());
        Ok(temp)
    }

    /// Move a staged body into its final location, backing up any previous file.
    fn publish(&mut self, temp: &Path, final_path: &Path) -> Result<()> {
        let backup = if final_path.exists() {
            let backup = final_path.with_extension("md.bak-tmp");
            std::fs::rename(final_path, &backup)?;
            Some(backup)
        } else {
            None
        };
        std::fs::rename(temp, final_path)?;
        self.staged.retain(|t| t != temp);
        self.published.push((final_path.to_path_buf(), backup));
        Ok(())
    }

    /// Make the publishes permanent: drop backups and any unused staged files.
    fn commit(mut self) {
        for (_, backup) in &self.published {
            if let Some(backup) = backup {
                let _ = std::fs::remove_file(backup);
            }
        }
        for temp in &self.staged {
            let _ = std::fs::remove_file(temp);
        }
        self.committed = true;
    }
}

impl Drop for BodyTransaction {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Undo in reverse so a path published twice ends on its oldest backup.
        for (final_path, backup) in self.published.iter().rev() {
            let _ = std::fs::remove_file(final_path);
            if let Some(backup) = backup {
                let _ = std::fs::rename(backup, final_path);
            }
        }
        for temp in &self.staged {
            let _ = std::fs::remove_file(temp);
        }
    }
}

// ── Store trait ─────────────────────────────────────────────────────────

/// Storage operations for distilled project knowledge.
pub trait KnowledgeStore: Send + Sync {
    /// Create tables/indexes if missing.
    fn init(&self) -> Result<()>;

    /// Generate a new unique page ID (e.g. `cas-kn001`).
    fn generate_id(&self) -> Result<String>;

    /// Directory holding markdown bodies.
    fn knowledge_dir(&self) -> PathBuf;

    /// Absolute path of a page body on disk.
    fn body_path(&self, rel_path: &str) -> Result<PathBuf>;

    /// Read a page body from disk.
    fn read_body(&self, rel_path: &str) -> Result<String>;

    /// Apply one distillation pass atomically: page rows, FTS index rows,
    /// source-ledger transitions and tombstone removal all commit together.
    fn commit_ingest(&self, batch: &IngestBatch) -> Result<IngestReport>;

    /// Fetch a page by its ID.
    fn get_page(&self, id: &str) -> Result<KnowledgePage>;

    /// Fetch a page by its canonical relative path.
    fn get_page_by_rel_path(&self, rel_path: &str) -> Result<Option<KnowledgePage>>;

    /// All pages, ordered by type then title (the injectable index).
    fn list_pages(&self) -> Result<Vec<KnowledgePage>>;

    /// Pages still awaiting an embedding.
    fn list_pending_embedding(&self, limit: usize) -> Result<Vec<KnowledgePage>>;

    /// How many pages are still awaiting an embedding, store-wide.
    ///
    /// Deliberately a count and not `list_pending_embedding(usize::MAX).len()`:
    /// this is the number a sync run reports as still-uncovered, so it must not
    /// be bounded by any caller's page budget, and it must not pay for
    /// materializing every row to answer.
    fn count_pending_embedding(&self) -> Result<usize>;

    /// Clear the `pending_embedding` flag once an embedding has been computed.
    fn mark_embedded(&self, id: &str) -> Result<()>;

    /// Re-arm `pending_embedding` on every page.
    ///
    /// Used when the embedding model changes: vectors from two different
    /// models are not comparable, so the cached ones are discarded and every
    /// page has to be embedded again — including pages that were previously
    /// marked done. Returns the number of pages re-armed.
    fn mark_all_pending_embedding(&self) -> Result<usize>;

    /// Set or clear the user-sovereignty lock on an existing page.
    ///
    /// This is the only way to lock a page after creation: `commit_ingest`
    /// deliberately never touches `locked` on an update, so distillation can
    /// neither lock nor unlock what the user decided.
    fn set_locked(&self, id: &str, locked: bool) -> Result<()>;

    /// Delete a page: row, FTS entry and body file.
    fn delete_page(&self, id: &str) -> Result<()>;

    /// Tombstones this machine authored but has not yet sent to the cloud.
    fn list_pending_page_tombstones(&self) -> Result<Vec<KnowledgePageTombstone>>;

    /// Mark the listed local tombstones as delivered after their push request
    /// succeeded. The tombstone itself remains as a no-resurrection guard.
    fn mark_page_tombstones_pushed(&self, ids: &[String]) -> Result<()>;

    /// Apply one remote tombstone. A locked page is never deleted here;
    /// deleting it later requires explicit local operator action.
    fn apply_remote_page_tombstone(
        &self,
        id: &str,
        deleted_at: DateTime<Utc>,
    ) -> Result<TombstoneApplyOutcome>;

    /// Whether a durable tombstone blocks an incoming page record.
    fn is_page_tombstoned(&self, id: &str) -> Result<bool>;

    /// Full-text search across title + snippet + body.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<KnowledgeHit>>;

    /// The whole source ledger, ordered by path.
    fn list_sources(&self) -> Result<Vec<KnowledgeSource>>;

    /// One ledger row.
    fn get_source(&self, file_path: &str) -> Result<Option<KnowledgeSource>>;

    /// Pages whose provenance cites `file_path`. This is how the distillation
    /// pipeline finds the pages to merge when a source file changes.
    fn pages_for_source(&self, file_path: &str) -> Result<Vec<KnowledgePage>>;

    /// Classify the on-disk source set against the current ledger.
    fn classify(&self, disk: &[DiskSource]) -> Result<SourceClassification> {
        Ok(classify_sources(disk, &self.list_sources()?))
    }
}

// ── SQLite implementation ───────────────────────────────────────────────

pub struct SqliteKnowledgeStore {
    conn: Arc<Mutex<Connection>>,
    cas_dir: PathBuf,
}

impl SqliteKnowledgeStore {
    /// Open (and initialize) the knowledge store rooted at `cas_dir`.
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self {
            conn,
            cas_dir: cas_dir.to_path_buf(),
        };
        store.init()?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn parse_datetime(value: &str) -> DateTime<Utc> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
            return dt.with_timezone(&Utc);
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
            return Utc.from_utc_datetime(&dt);
        }
        Utc::now()
    }

    fn parse_sources(value: &str) -> Vec<String> {
        if value.is_empty() {
            return Vec::new();
        }
        serde_json::from_str(value).unwrap_or_default()
    }

    fn page_from_row(row: &rusqlite::Row) -> rusqlite::Result<KnowledgePage> {
        Ok(KnowledgePage {
            id: row.get("id")?,
            page_type: row.get("page_type")?,
            title: row.get("title")?,
            rel_path: row.get("rel_path")?,
            snippet: row.get("snippet")?,
            locked: row.get::<_, i64>("locked")? != 0,
            sources: Self::parse_sources(&row.get::<_, String>("sources_json")?),
            origin: row
                .get::<_, String>("origin")?
                .parse()
                .map_err(|e: StoreError| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
            origin_project_id: row.get("origin_project_id")?,
            created_at: Self::parse_datetime(&row.get::<_, String>("created_at")?),
            updated_at: Self::parse_datetime(&row.get::<_, String>("updated_at")?),
            pending_embedding: row.get::<_, i64>("pending_embedding")? != 0,
        })
    }

    fn source_from_row(row: &rusqlite::Row) -> rusqlite::Result<KnowledgeSource> {
        Ok(KnowledgeSource {
            file_path: row.get("file_path")?,
            blake3: row.get("blake3")?,
            size: row.get::<_, i64>("size")?.max(0) as u64,
            status: row
                .get::<_, String>("status")?
                .parse()
                .unwrap_or(SourceStatus::Uploaded),
            ingest_error: row.get("ingest_error")?,
            created_at: Self::parse_datetime(&row.get::<_, String>("created_at")?),
            updated_at: Self::parse_datetime(&row.get::<_, String>("updated_at")?),
        })
    }

    const PAGE_COLUMNS: &'static str = "id, page_type, title, rel_path, snippet, locked, \
                                        sources_json, origin, origin_project_id, created_at, \
                                        updated_at, pending_embedding";

    const SOURCE_COLUMNS: &'static str =
        "file_path, blake3, size, status, ingest_error, created_at, updated_at";

    /// Turn a free-text query into an FTS5 expression that cannot be a syntax
    /// error: every token is quoted, and terms are **ORed**.
    ///
    /// WHY OR AND NOT AND (cas-461a): this used to join the quoted tokens with
    /// a space, which FTS5 reads as an implicit `AND` — every term had to occur
    /// in the same page. The surface this replaces (Tantivy BM25 over `entries`)
    /// is disjunctive, so the two sides differed in boolean semantics and the
    /// knowledge side was the strict one: recall fell as the query got longer,
    /// and past ~3 terms a user got nothing. The cas-d075 measurement
    /// (`docs/migration/cas-b129-knowledge-retrieval-verdict.md`) found 7 of 10
    /// real-vocabulary queries returning **zero** pages where legacy returned
    /// 4–10, with the same queries matching 18–107 pages under `OR` — proving
    /// the content was present and indexed and the defect was query
    /// construction alone. The failure was silent (a clean "no matches", not an
    /// error), which is why it survived.
    ///
    /// Ranking is unaffected and needs no extra machinery: `search` already
    /// orders by `bm25()`, and BM25 over a disjunctive match set inherently
    /// prefers pages containing more of the query's terms. So an OR match with
    /// BM25 ordering *is* the "AND-preference" behaviour, without the recall
    /// cliff.
    ///
    /// Double-quoted runs in the user's query are preserved as FTS5 phrases:
    /// `verifier "quality gates"` becomes `"verifier" OR "quality gates"`.
    /// An unterminated quote is treated as a phrase running to end of input
    /// rather than an error.
    ///
    /// Injection safety is unchanged and structural: only `[a-z0-9]` tokens
    /// ever reach the output, so no user input can close a quote or introduce
    /// an FTS5 operator. Punctuation-only input yields `None`, which `search`
    /// turns into an empty result set instead of a syntax error.
    ///
    /// The implementation moved to [`crate::fts_query::fts_or_query`] when the
    /// history index became the second FTS consumer (EPIC cas-6212 / cas-7f40).
    /// This stays as a named delegate so the reasoning above keeps living next
    /// to the store that first needed it.
    fn fts_query(query: &str) -> Option<String> {
        crate::fts_query::fts_or_query(query)
    }

    /// Confirm a resolved directory really lives inside the knowledge dir.
    ///
    /// [`validate_rel_path`] is lexical, so it cannot see a symlinked component
    /// under `.cas/knowledge/`. Page paths are derived from LLM output, so the
    /// resolved path gets checked too before anything is written through it.
    fn ensure_within_knowledge_dir(&self, dir: &Path) -> Result<()> {
        let root = self.knowledge_dir();
        let root = root.canonicalize().unwrap_or(root);
        let resolved = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !resolved.starts_with(&root) {
            return Err(StoreError::Parse(format!(
                "knowledge body path escapes the knowledge directory: {resolved:?}"
            )));
        }
        Ok(())
    }

    /// Replace a page's FTS row. Called only inside the ingest transaction.
    fn reindex_page(
        conn: &Connection,
        row_id: i64,
        title: &str,
        snippet: &str,
        body: &str,
    ) -> Result<()> {
        conn.execute(
            "DELETE FROM knowledge_pages_fts WHERE rowid = ?1",
            params![row_id],
        )?;
        conn.execute(
            "INSERT INTO knowledge_pages_fts (rowid, title, snippet, body)
             VALUES (?1, ?2, ?3, ?4)",
            params![row_id, title, snippet, body],
        )?;
        Ok(())
    }
}

impl KnowledgeStore for SqliteKnowledgeStore {
    fn init(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(KNOWLEDGE_SCHEMA)?;
        Ok(())
    }

    /// Sequence-backed, not hash-backed. A distillation pass mints several IDs
    /// *before* any of them is inserted, so a hash-plus-existence-check scheme
    /// (as in `skill_store`) can hand out the same ID twice within one batch and
    /// blow up on the UNIQUE constraint at commit time.
    ///
    /// Uniqueness comes from the atomic `id_sequences` counter plus a bounded
    /// repair step: if the counter is behind the table (a restored backup, a
    /// hand-edited DB), the sequence is fast-forwarded past the highest id in
    /// use rather than stepped one at a time. Mirrors `rule_store`.
    fn generate_id(&self) -> Result<String> {
        let conn = self.lock();
        for _ in 0..8 {
            let next = shared_db::next_sequence_val(&conn, "knowledge_page")?;
            let id = format!("cas-kn{next:03}");
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM knowledge_pages WHERE id = ?1)",
                params![&id],
                |row| row.get(0),
            )?;
            if !exists {
                return Ok(id);
            }

            // Stale sequence: jump it past the highest id actually in use so
            // the next draw is free, instead of stepping through the whole gap.
            let max_used: i64 = conn.query_row(
                "SELECT COALESCE(MAX(CASE WHEN id GLOB 'cas-kn[0-9]*'
                     THEN CAST(SUBSTR(id, 7) AS INTEGER) END), 0)
                 FROM knowledge_pages",
                [],
                |row| row.get(0),
            )?;
            if max_used >= next {
                conn.execute(
                    "UPDATE id_sequences SET next_val = ?1 WHERE name = 'knowledge_page'",
                    params![max_used],
                )?;
            }
        }
        Err(StoreError::Other(
            "could not allocate a free knowledge page ID".to_string(),
        ))
    }

    fn knowledge_dir(&self) -> PathBuf {
        self.cas_dir.join(KNOWLEDGE_DIR_NAME)
    }

    fn body_path(&self, rel_path: &str) -> Result<PathBuf> {
        validate_rel_path(rel_path)?;
        Ok(self.knowledge_dir().join(rel_path))
    }

    fn read_body(&self, rel_path: &str) -> Result<String> {
        let path = self.body_path(rel_path)?;
        std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(format!("knowledge body not found: {rel_path}"))
            } else {
                StoreError::Io(e)
            }
        })
    }

    fn commit_ingest(&self, batch: &IngestBatch) -> Result<IngestReport> {
        // 1. Validate everything before touching disk or the DB.
        for write in &batch.pages {
            validate_rel_path(&write.page.rel_path)?;
            if write.page.id.trim().is_empty() {
                return Err(StoreError::Parse(
                    "knowledge page id must not be empty".to_string(),
                ));
            }
            match write.page.origin {
                KnowledgePageOrigin::Local if write.page.origin_project_id.is_some() => {
                    return Err(StoreError::Parse(format!(
                        "local knowledge page {} must not claim an origin project id",
                        write.page.id
                    )));
                }
                KnowledgePageOrigin::CloudPull
                    if write
                        .page
                        .origin_project_id
                        .as_deref()
                        .is_none_or(str::is_empty) =>
                {
                    return Err(StoreError::Parse(format!(
                        "cloud-pulled knowledge page {} requires an origin project id",
                        write.page.id
                    )));
                }
                _ => {}
            }
        }

        // 2. Stage every body as a temp file with NO database lock held —
        //    `shared_db` hands out one connection per process, so doing batch
        //    file I/O under that mutex would stall every unrelated store.
        //    Nothing is published to its final path yet.
        let mut bodies = BodyTransaction::new();
        let mut staged: Vec<(&PageWrite, PathBuf, PathBuf)> = Vec::new();
        for (index, write) in batch.pages.iter().enumerate() {
            let final_path = self.body_path(&write.page.rel_path)?;
            if let Some(parent) = final_path.parent() {
                std::fs::create_dir_all(parent)?;
                // Lexical validation cannot see a symlinked component, so
                // confirm the resolved directory really is inside the
                // knowledge dir before writing LLM-named paths into it.
                self.ensure_within_knowledge_dir(parent)?;
            }
            let temp = bodies.stage(&final_path, index, &write.body)?;
            staged.push((write, temp, final_path));
        }

        // 3. ONE transaction for page rows, FTS index rows, ledger transitions
        //    and the tombstone cascade. Body publishes are interleaved but are
        //    themselves transactional (see `BodyTransaction`), so an abort
        //    rolls back the database AND the filesystem together.
        let conn = self.lock();
        let tx = ImmediateTx::new(&conn).map_err(StoreError::Database)?;
        let mut report = IngestReport::default();
        let mut orphan_bodies: Vec<String> = Vec::new();

        for (write, temp, final_path) in &staged {
            let page = &write.page;

            // A tombstone is stronger than an old page record. This check is
            // inside the write transaction (rather than only in cloud pull)
            // so every producer — including a stale local process — gets the
            // same no-resurrection rule.
            let tombstoned: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM knowledge_page_tombstones WHERE id = ?1)",
                params![page.id],
                |row| row.get(0),
            )?;
            if tombstoned {
                report.tombstoned_skipped_page_ids.push(page.id.clone());
                continue;
            }

            // A page id that already belongs to a DIFFERENT path would abort
            // the pass on the UNIQUE(id) constraint with an opaque SQLite
            // error. Fail fast and say which pair collided instead.
            let claimed: Option<String> = tx
                .query_row(
                    "SELECT rel_path FROM knowledge_pages WHERE id = ?1",
                    params![page.id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = claimed {
                if existing != page.rel_path {
                    return Err(StoreError::Parse(format!(
                        "knowledge page id {} already belongs to {existing}, cannot reuse it for {}",
                        page.id, page.rel_path
                    )));
                }
            }

            let sources_json = serde_json::to_string(&page.sources)?;
            let created = page.created_at.to_rfc3339();
            let updated = page.updated_at.to_rfc3339();

            // `locked` is deliberately absent from the DO UPDATE set-list and
            // guarded by the WHERE: distillation can neither overwrite a locked
            // page nor lock one that the user did not lock.
            let row_id: Option<i64> = tx
                .query_row(
                    "INSERT INTO knowledge_pages
                        (id, page_type, title, rel_path, snippet, locked, sources_json,
                         origin, origin_project_id, created_at, updated_at, pending_embedding)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(rel_path) DO UPDATE SET
                        page_type = excluded.page_type,
                        title = excluded.title,
                        snippet = excluded.snippet,
                        sources_json = excluded.sources_json,
                        origin = excluded.origin,
                        origin_project_id = excluded.origin_project_id,
                        updated_at = excluded.updated_at,
                        pending_embedding = excluded.pending_embedding
                     WHERE knowledge_pages.locked = 0
                     RETURNING row_id",
                    params![
                        page.id,
                        page.page_type,
                        page.title,
                        page.rel_path,
                        page.snippet,
                        page.locked as i64,
                        sources_json,
                        page.origin.as_str(),
                        page.origin_project_id,
                        created,
                        updated,
                        page.pending_embedding as i64,
                    ],
                    |row| row.get(0),
                )
                .optional()?;

            // No row means the existing page is locked. The staged body is
            // discarded and the user's file on disk is never touched.
            let Some(row_id) = row_id else {
                report.locked_skipped_rel_paths.push(page.rel_path.clone());
                continue;
            };

            Self::reindex_page(&tx, row_id, &page.title, &page.snippet, &write.body)?;
            bodies.publish(temp, final_path)?;
            report.pages_written += 1;
        }

        for outcome in &batch.sources {
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "INSERT INTO knowledge_sources
                    (file_path, blake3, size, status, ingest_error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(file_path) DO UPDATE SET
                    blake3 = excluded.blake3,
                    size = excluded.size,
                    status = excluded.status,
                    ingest_error = excluded.ingest_error,
                    updated_at = excluded.updated_at",
                params![
                    outcome.file_path,
                    outcome.blake3,
                    outcome.size as i64,
                    outcome.status.as_str(),
                    outcome.ingest_error,
                    now,
                ],
            )?;
            report.sources_recorded += 1;
        }

        for path in &batch.tombstones {
            let removed = tx.execute(
                "DELETE FROM knowledge_sources WHERE file_path = ?1",
                params![path],
            )?;
            report.sources_tombstoned += removed;

            // Cascade by provenance: strip the dead source from every page that
            // cites it; a page left with no provenance (and not locked) goes.
            let affected: Vec<(i64, String, String, String, i64)> = {
                let mut stmt = tx.prepare(PAGES_CITING_SOURCE_SQL)?;
                stmt.query_map(params![json_fragment(path)], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };

            for (row_id, page_id, rel_path, sources_json, locked) in affected {
                let mut sources = Self::parse_sources(&sources_json);
                let before = sources.len();
                sources.retain(|s| s != path);
                if sources.len() == before {
                    continue; // LIKE false positive (substring of another path)
                }

                // A locked page keeps its row AND its body even when its last
                // source dies — the user owns it now, not the distiller.
                if sources.is_empty() && locked == 0 {
                    let deleted_at = Utc::now().to_rfc3339();
                    tx.execute(
                        "INSERT INTO knowledge_page_tombstones
                            (id, deleted_at, locally_authored, pushed_at)
                         VALUES (?1, ?2, 1, NULL)
                         ON CONFLICT(id) DO UPDATE SET
                            deleted_at = excluded.deleted_at,
                            locally_authored = 1,
                            pushed_at = NULL",
                        params![page_id, deleted_at],
                    )?;
                    tx.execute(
                        "DELETE FROM knowledge_pages WHERE row_id = ?1",
                        params![row_id],
                    )?;
                    tx.execute(
                        "DELETE FROM knowledge_pages_fts WHERE rowid = ?1",
                        params![row_id],
                    )?;
                    report.cascade_deleted_page_ids.push(page_id);
                    orphan_bodies.push(rel_path);
                } else {
                    tx.execute(
                        "UPDATE knowledge_pages SET sources_json = ?1, updated_at = ?2
                         WHERE row_id = ?3",
                        params![
                            serde_json::to_string(&sources)?,
                            Utc::now().to_rfc3339(),
                            row_id
                        ],
                    )?;
                }
            }
        }

        tx.commit().map_err(StoreError::Database)?;
        bodies.commit();
        drop(conn);

        // 4. Only after the commit do we unlink bodies for cascade-deleted
        //    pages. A failure here leaves an orphan file, never a dangling row.
        for rel_path in orphan_bodies {
            if let Ok(path) = self.body_path(&rel_path) {
                let _ = std::fs::remove_file(path);
            }
        }

        Ok(report)
    }

    fn get_page(&self, id: &str) -> Result<KnowledgePage> {
        let conn = self.lock();
        conn.query_row(
            &format!(
                "SELECT {} FROM knowledge_pages WHERE id = ?1",
                Self::PAGE_COLUMNS
            ),
            params![id],
            Self::page_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("knowledge page not found: {id}")))
    }

    fn get_page_by_rel_path(&self, rel_path: &str) -> Result<Option<KnowledgePage>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {} FROM knowledge_pages WHERE rel_path = ?1",
                    Self::PAGE_COLUMNS
                ),
                params![rel_path],
                Self::page_from_row,
            )
            .optional()?)
    }

    fn list_pages(&self) -> Result<Vec<KnowledgePage>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM knowledge_pages ORDER BY page_type, title",
            Self::PAGE_COLUMNS
        ))?;
        let pages = stmt
            .query_map([], Self::page_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    fn list_pending_embedding(&self, limit: usize) -> Result<Vec<KnowledgePage>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM knowledge_pages WHERE pending_embedding = 1
             ORDER BY updated_at LIMIT ?1",
            Self::PAGE_COLUMNS
        ))?;
        let pages = stmt
            .query_map(params![limit as i64], Self::page_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    fn count_pending_embedding(&self) -> Result<usize> {
        let conn = self.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM knowledge_pages WHERE pending_embedding = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    fn mark_embedded(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        let rows = conn.execute(
            "UPDATE knowledge_pages SET pending_embedding = 0 WHERE id = ?1",
            params![id],
        )?;
        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "knowledge page not found: {id}"
            )));
        }
        Ok(())
    }

    fn mark_all_pending_embedding(&self) -> Result<usize> {
        let conn = self.lock();
        // `updated_at` is deliberately NOT bumped: re-embedding is an internal
        // cache concern, and touching the timestamp would make every page look
        // freshly edited to sync conflict resolution.
        let rows = conn.execute(
            "UPDATE knowledge_pages SET pending_embedding = 1 WHERE pending_embedding = 0",
            [],
        )?;
        Ok(rows)
    }

    fn set_locked(&self, id: &str, locked: bool) -> Result<()> {
        let conn = self.lock();
        let rows = conn.execute(
            "UPDATE knowledge_pages SET locked = ?1, updated_at = ?2 WHERE id = ?3",
            params![locked as i64, Utc::now().to_rfc3339(), id],
        )?;
        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "knowledge page not found: {id}"
            )));
        }
        Ok(())
    }

    fn delete_page(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        let tx = ImmediateTx::new(&conn).map_err(StoreError::Database)?;
        let found: Option<(i64, String)> = tx
            .query_row(
                "SELECT row_id, rel_path FROM knowledge_pages WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((row_id, rel_path)) = found else {
            return Err(StoreError::NotFound(format!(
                "knowledge page not found: {id}"
            )));
        };
        tx.execute(
            "INSERT INTO knowledge_page_tombstones
                (id, deleted_at, locally_authored, pushed_at)
             VALUES (?1, ?2, 1, NULL)
             ON CONFLICT(id) DO UPDATE SET
                deleted_at = excluded.deleted_at,
                locally_authored = 1,
                pushed_at = NULL",
            params![id, Utc::now().to_rfc3339()],
        )?;
        tx.execute(
            "DELETE FROM knowledge_pages WHERE row_id = ?1",
            params![row_id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_pages_fts WHERE rowid = ?1",
            params![row_id],
        )?;
        tx.commit().map_err(StoreError::Database)?;

        if let Ok(path) = self.body_path(&rel_path) {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }

    fn list_pending_page_tombstones(&self) -> Result<Vec<KnowledgePageTombstone>> {
        let conn = self.lock();
        let mut statement = conn.prepare(
            "SELECT id, deleted_at, locally_authored
             FROM knowledge_page_tombstones
             WHERE locally_authored = 1 AND pushed_at IS NULL
             ORDER BY deleted_at, id",
        )?;
        statement
            .query_map([], |row| {
                let deleted_at: String = row.get(1)?;
                let deleted_at = DateTime::parse_from_rfc3339(&deleted_at)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc);
                Ok(KnowledgePageTombstone {
                    id: row.get(0)?,
                    deleted_at,
                    locally_authored: row.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    fn mark_page_tombstones_pushed(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.lock();
        let tx = ImmediateTx::new(&conn).map_err(StoreError::Database)?;
        let pushed_at = Utc::now().to_rfc3339();
        for id in ids {
            tx.execute(
                "UPDATE knowledge_page_tombstones
                 SET pushed_at = ?1
                 WHERE id = ?2 AND locally_authored = 1 AND pushed_at IS NULL",
                params![pushed_at, id],
            )?;
        }
        tx.commit().map_err(StoreError::Database)
    }

    fn apply_remote_page_tombstone(
        &self,
        id: &str,
        deleted_at: DateTime<Utc>,
    ) -> Result<TombstoneApplyOutcome> {
        let conn = self.lock();
        let tx = ImmediateTx::new(&conn).map_err(StoreError::Database)?;
        let page: Option<(i64, String, i64)> = tx
            .query_row(
                "SELECT row_id, rel_path, locked FROM knowledge_pages WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        // Preserve local authorship/push state if this machine already emitted
        // the deletion; a server echo must never turn a local tombstone into a
        // remote-only one. Retain the newest timestamp for auditability.
        let deleted_at = deleted_at.to_rfc3339();
        tx.execute(
            "INSERT INTO knowledge_page_tombstones
                (id, deleted_at, locally_authored, pushed_at)
             VALUES (?1, ?2, 0, NULL)
             ON CONFLICT(id) DO UPDATE SET
                deleted_at = CASE
                    WHEN excluded.deleted_at > knowledge_page_tombstones.deleted_at
                    THEN excluded.deleted_at
                    ELSE knowledge_page_tombstones.deleted_at
                END",
            params![id, deleted_at],
        )?;

        let outcome = match page {
            Some((_row_id, _rel_path, locked)) if locked != 0 => {
                TombstoneApplyOutcome::LockedPreserved
            }
            Some((row_id, rel_path, _)) => {
                tx.execute(
                    "DELETE FROM knowledge_pages WHERE row_id = ?1",
                    params![row_id],
                )?;
                tx.execute(
                    "DELETE FROM knowledge_pages_fts WHERE rowid = ?1",
                    params![row_id],
                )?;
                tx.commit().map_err(StoreError::Database)?;
                if let Ok(path) = self.body_path(&rel_path) {
                    let _ = std::fs::remove_file(path);
                }
                return Ok(TombstoneApplyOutcome::Applied);
            }
            None => TombstoneApplyOutcome::Applied,
        };
        tx.commit().map_err(StoreError::Database)?;
        Ok(outcome)
    }

    fn is_page_tombstoned(&self, id: &str) -> Result<bool> {
        let conn = self.lock();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM knowledge_page_tombstones WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )
        .map_err(StoreError::Database)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<KnowledgeHit>> {
        let Some(expr) = Self::fts_query(query) else {
            return Ok(Vec::new());
        };
        let conn = self.lock();
        // `knowledge_pages.*` rather than a second hand-maintained column list:
        // the qualifier resolves the title/snippet ambiguity with the FTS
        // columns, and `page_from_row` reads by name so the extra `row_id` is
        // harmless. One column list, no drift.
        let mut stmt = conn.prepare(
            "SELECT knowledge_pages.*, bm25(knowledge_pages_fts) AS score
             FROM knowledge_pages_fts
             JOIN knowledge_pages ON knowledge_pages.row_id = knowledge_pages_fts.rowid
             WHERE knowledge_pages_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;
        let hits = stmt
            .query_map(params![expr, limit as i64], |row| {
                Ok(KnowledgeHit {
                    page: Self::page_from_row(row)?,
                    score: row.get("score")?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(hits)
    }

    fn list_sources(&self) -> Result<Vec<KnowledgeSource>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM knowledge_sources ORDER BY file_path",
            Self::SOURCE_COLUMNS
        ))?;
        let sources = stmt
            .query_map([], Self::source_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(sources)
    }

    fn pages_for_source(&self, file_path: &str) -> Result<Vec<KnowledgePage>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM knowledge_pages WHERE sources_json LIKE '%' || ?1 || '%'",
            Self::PAGE_COLUMNS
        ))?;
        let pages = stmt
            .query_map(params![json_fragment(file_path)], Self::page_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // The LIKE is only a prefilter — confirm exact membership.
        Ok(pages
            .into_iter()
            .filter(|p| p.sources.iter().any(|s| s == file_path))
            .collect())
    }

    fn get_source(&self, file_path: &str) -> Result<Option<KnowledgeSource>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {} FROM knowledge_sources WHERE file_path = ?1",
                    Self::SOURCE_COLUMNS
                ),
                params![file_path],
                Self::source_from_row,
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Pure classifier (no I/O, no store) ──────────────────────────────

    fn ledger_row(path: &str, hash: &str, size: u64, status: SourceStatus) -> KnowledgeSource {
        KnowledgeSource {
            file_path: path.to_string(),
            blake3: hash.to_string(),
            size,
            status,
            ingest_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn disk_row(path: &str, hash: &str, size: u64) -> DiskSource {
        DiskSource {
            file_path: path.to_string(),
            blake3: hash.to_string(),
            size,
        }
    }

    #[test]
    fn classify_marks_unknown_sources_as_new() {
        let disk = vec![disk_row("src/a.rs", "aaa", 10)];
        let result = classify_sources(&disk, &[]);
        assert_eq!(result.to_ingest.len(), 1);
        assert_eq!(result.to_ingest[0].reason, IngestReason::New);
        assert!(result.skipped.is_empty());
        assert!(result.deleted.is_empty());
    }

    #[test]
    fn classify_skips_unchanged_ingested_sources() {
        let disk = vec![disk_row("src/a.rs", "aaa", 10)];
        let ledger = vec![ledger_row("src/a.rs", "aaa", 10, SourceStatus::Ingested)];
        let result = classify_sources(&disk, &ledger);
        assert!(result.to_ingest.is_empty());
        assert_eq!(result.skipped, disk);
        assert!(result.deleted.is_empty());
    }

    #[test]
    fn classify_reingests_on_hash_change() {
        let disk = vec![disk_row("src/a.rs", "bbb", 10)];
        let ledger = vec![ledger_row("src/a.rs", "aaa", 10, SourceStatus::Ingested)];
        let result = classify_sources(&disk, &ledger);
        assert_eq!(result.to_ingest[0].reason, IngestReason::Changed);
    }

    #[test]
    fn classify_reingests_on_size_change_with_same_hash() {
        // Defensive: a hash collision or a truncated read must not be skipped.
        let disk = vec![disk_row("src/a.rs", "aaa", 11)];
        let ledger = vec![ledger_row("src/a.rs", "aaa", 10, SourceStatus::Ingested)];
        let result = classify_sources(&disk, &ledger);
        assert_eq!(result.to_ingest[0].reason, IngestReason::Changed);
    }

    #[test]
    fn classify_auto_retries_failed_and_uploaded_rows() {
        let disk = vec![
            disk_row("src/a.rs", "aaa", 10),
            disk_row("src/b.rs", "bbb", 20),
        ];
        let ledger = vec![
            ledger_row("src/a.rs", "aaa", 10, SourceStatus::Failed),
            ledger_row("src/b.rs", "bbb", 20, SourceStatus::Uploaded),
        ];
        let result = classify_sources(&disk, &ledger);
        assert_eq!(result.to_ingest.len(), 2);
        assert!(
            result
                .to_ingest
                .iter()
                .all(|p| p.reason == IngestReason::Retry)
        );
        assert!(result.skipped.is_empty());
    }

    #[test]
    fn classify_tombstones_ledger_rows_missing_from_disk() {
        let disk = vec![disk_row("src/a.rs", "aaa", 10)];
        let ledger = vec![
            ledger_row("src/a.rs", "aaa", 10, SourceStatus::Ingested),
            ledger_row("src/gone.rs", "ggg", 5, SourceStatus::Ingested),
        ];
        let result = classify_sources(&disk, &ledger);
        assert_eq!(result.deleted, vec!["src/gone.rs".to_string()]);
    }

    #[test]
    fn classify_collapses_duplicate_disk_entries() {
        let disk = vec![
            disk_row("src/a.rs", "aaa", 10),
            disk_row("src/a.rs", "aaa", 10),
        ];
        let result = classify_sources(&disk, &[]);
        assert_eq!(result.to_ingest.len(), 1);
    }

    #[test]
    fn classify_is_deterministic_and_total() {
        let disk = vec![
            disk_row("z.rs", "z", 1),
            disk_row("a.rs", "a", 1),
            disk_row("m.rs", "m", 1),
        ];
        let ledger = vec![ledger_row("a.rs", "a", 1, SourceStatus::Ingested)];
        let first = classify_sources(&disk, &ledger);
        let second = classify_sources(&disk, &ledger);
        assert_eq!(first, second);
        // Every disk entry lands in exactly one bucket.
        assert_eq!(first.to_ingest.len() + first.skipped.len(), disk.len());
    }

    // ── Slug / path canonicalization ────────────────────────────────────

    #[test]
    fn canonical_path_merges_equivalent_titles() {
        assert_eq!(
            canonical_rel_path("Architecture", "The Build System!"),
            canonical_rel_path("architecture", "the  build   system")
        );
        assert_eq!(
            canonical_rel_path("architecture", "Build System"),
            "architecture/build-system.md"
        );
    }

    #[test]
    fn canonical_path_survives_empty_input() {
        assert_eq!(canonical_rel_path("", ""), "general/untitled.md");
    }

    #[test]
    fn rel_path_rejects_traversal_and_absolute_paths() {
        for bad in [
            "../escape.md",
            "/etc/passwd.md",
            "a/../../b.md",
            "notes.txt",
            "",
            "win\\path.md",
        ] {
            assert!(
                validate_rel_path(bad).is_err(),
                "expected rejection of {bad:?}"
            );
        }
        assert!(validate_rel_path("architecture/build-system.md").is_ok());
    }

    // ── Store round-trips ───────────────────────────────────────────────

    fn store() -> (TempDir, SqliteKnowledgeStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteKnowledgeStore::open(temp.path()).unwrap();
        (temp, store)
    }

    fn page(
        store: &SqliteKnowledgeStore,
        page_type: &str,
        title: &str,
        sources: &[&str],
    ) -> KnowledgePage {
        let mut p = KnowledgePage::new(store.generate_id().unwrap(), page_type, title);
        p.snippet = format!("summary of {title}");
        p.sources = sources.iter().map(|s| s.to_string()).collect();
        p
    }

    #[test]
    fn ingest_writes_body_to_disk_and_row_to_db() {
        let (_temp, store) = store();
        let p = page(&store, "architecture", "Build System", &["Cargo.toml"]);
        let batch = IngestBatch {
            pages: vec![PageWrite {
                page: p.clone(),
                body: "# Build System\n\nCargo workspace with mold linker.".to_string(),
            }],
            sources: vec![SourceOutcome {
                file_path: "Cargo.toml".to_string(),
                blake3: "aaa".to_string(),
                size: 12,
                status: SourceStatus::Ingested,
                ingest_error: None,
            }],
            tombstones: vec![],
        };

        let report = store.commit_ingest(&batch).unwrap();
        assert_eq!(report.pages_written, 1);
        assert_eq!(report.sources_recorded, 1);

        // Body is a real, greppable markdown file on disk.
        let body_path = store.body_path(&p.rel_path).unwrap();
        assert!(body_path.exists(), "body must exist at {body_path:?}");
        let on_disk = std::fs::read_to_string(&body_path).unwrap();
        assert!(on_disk.contains("mold linker"));
        assert_eq!(store.read_body(&p.rel_path).unwrap(), on_disk);

        // The body text is NOT stored in any SQLite column.
        let conn = store.lock();
        let stored: String = conn
            .query_row(
                "SELECT group_concat(snippet || sources_json || title) FROM knowledge_pages",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!stored.contains("mold linker"));
        drop(conn);

        let fetched = store.get_page(&p.id).unwrap();
        assert_eq!(fetched.title, "Build System");
        assert_eq!(fetched.sources, vec!["Cargo.toml".to_string()]);
        assert_eq!(fetched.origin, KnowledgePageOrigin::Local);
        assert_eq!(fetched.origin_project_id, None);
        assert!(fetched.pending_embedding);
        assert_eq!(store.list_pending_embedding(10).unwrap().len(), 1);

        let source = store.get_source("Cargo.toml").unwrap().unwrap();
        assert_eq!(source.status, SourceStatus::Ingested);
    }

    #[test]
    fn cloud_pull_origin_requires_a_nonempty_project_identity() {
        let (_temp, store) = store();
        let mut p = page(&store, "architecture", "Remote", &[]);
        p.origin = KnowledgePageOrigin::CloudPull;
        let err = store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p,
                    body: "# Remote".to_string(),
                }],
                ..IngestBatch::default()
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("requires an origin project id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reingesting_same_subject_merges_instead_of_duplicating() {
        let (_temp, store) = store();
        let first = page(&store, "architecture", "Build System", &["Cargo.toml"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: first.clone(),
                    body: "v1 body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        // Same type + title, different generated ID and different casing.
        let second = page(
            &store,
            "Architecture",
            "build system",
            &["Cargo.toml", "Makefile"],
        );
        assert_eq!(first.rel_path, second.rel_path);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: second,
                    body: "v2 body with quorum".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        let pages = store.list_pages().unwrap();
        assert_eq!(pages.len(), 1, "canonical path must merge, not duplicate");
        // The original ID is preserved across the merge.
        assert_eq!(pages[0].id, first.id);
        assert_eq!(pages[0].sources.len(), 2);
        assert_eq!(
            store.read_body(&pages[0].rel_path).unwrap(),
            "v2 body with quorum"
        );

        // FTS reflects the new body only.
        assert_eq!(store.search("quorum", 10).unwrap().len(), 1);
        assert!(store.search("v1", 10).unwrap().is_empty());
    }

    #[test]
    fn locked_pages_are_never_overwritten_by_distillation() {
        let (_temp, store) = store();
        let mut p = page(&store, "workflow", "Release Process", &["docs/release.md"]);
        p.locked = true;
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "hand-written truth".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        let overwrite = page(&store, "workflow", "Release Process", &["docs/release.md"]);
        let report = store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: overwrite,
                    body: "llm guess".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(report.pages_written, 0);
        assert_eq!(report.locked_skipped_rel_paths, vec![p.rel_path.clone()]);
        assert_eq!(store.read_body(&p.rel_path).unwrap(), "hand-written truth");
    }

    #[test]
    fn search_matches_body_title_and_snippet() {
        let (_temp, store) = store();
        let a = page(&store, "subsystem", "Verifier", &["src/verify.rs"]);
        let b = page(&store, "subsystem", "Scheduler", &["src/sched.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![
                    PageWrite {
                        page: a.clone(),
                        body: "The verifier enforces quality gates before close.".to_string(),
                    },
                    PageWrite {
                        page: b.clone(),
                        body: "The scheduler assigns work to idle workers.".to_string(),
                    },
                ],
                ..Default::default()
            })
            .unwrap();

        let hits = store.search("quality gates", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page.id, a.id);

        // Title match.
        assert_eq!(store.search("Scheduler", 10).unwrap()[0].page.id, b.id);
        // Snippet match ("summary of Verifier").
        assert!(!store.search("summary", 10).unwrap().is_empty());
        // Punctuation-only / empty queries are safe, not syntax errors.
        assert!(store.search("", 10).unwrap().is_empty());
        assert!(store.search("\"( AND ) OR *", 10).unwrap().is_empty());
    }

    /// cas-461a. `fts_query` joined tokens with a space, which FTS5 reads as an
    /// implicit AND, so a multi-term query only matched pages containing
    /// *every* term. The cas-d075 measurement found 7 of 10 real queries
    /// returning zero pages where the legacy disjunctive surface returned 4–10.
    ///
    /// These assertions are on the constructed expression rather than only on
    /// results, because the defect was invisible in results — it produced a
    /// clean empty set, not an error.
    #[test]
    fn fts_query_is_disjunctive_and_preserves_explicit_phrases() {
        let q = |s: &str| SqliteKnowledgeStore::fts_query(s);

        // The regression itself: terms are ORed, never space-joined (AND).
        assert_eq!(
            q("cargo build tests check").unwrap(),
            "\"cargo\" OR \"build\" OR \"tests\" OR \"check\""
        );

        // Single term is unchanged.
        assert_eq!(q("widget").unwrap(), "\"widget\"");

        // An explicitly quoted run stays one adjacency-constrained phrase and
        // is not shattered into ORed words.
        assert_eq!(q("\"quality gates\"").unwrap(), "\"quality gates\"");
        assert_eq!(
            q("verifier \"quality gates\"").unwrap(),
            "\"verifier\" OR \"quality gates\""
        );

        // Case folding and punctuation splitting still apply inside phrases.
        assert_eq!(q("\"Quality-Gates\"").unwrap(), "\"quality gates\"");

        // An unterminated quote is a phrase to end of input, not an error.
        assert_eq!(
            q("open \"quality gates").unwrap(),
            "\"open\" OR \"quality gates\""
        );

        // Nothing but punctuation yields no expression at all, which `search`
        // maps to an empty result set rather than an FTS5 syntax error.
        assert!(q("").is_none());
        assert!(q("  -- ** ").is_none());

        // Injection safety is structural: only [a-z0-9] tokens reach the
        // output, so FTS5 operators typed by the user are inert data. Bare
        // operators become quoted literal terms...
        assert_eq!(
            q("( AND ) OR *").unwrap(),
            "\"and\" OR \"or\"",
            "operators must be quoted as literal terms, never emitted as syntax"
        );
        // ...and the same input behind an unterminated quote collapses into a
        // single literal phrase — still inert, no syntax escapes.
        assert_eq!(q("\"( AND ) OR *").unwrap(), "\"and or\"");
    }

    /// The behavioural half of cas-461a, through the store API: a query whose
    /// terms are spread across different pages must return those pages instead
    /// of the silent empty set the conjunction produced.
    #[test]
    fn search_returns_partial_term_matches_and_ranks_full_matches_first() {
        let (_temp, store) = store();
        let both = page(&store, "subsystem", "Both", &["both.rs"]);
        let only_cargo = page(&store, "subsystem", "OnlyCargo", &["cargo.rs"]);
        let only_verify = page(&store, "subsystem", "OnlyVerify", &["verify.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![
                    PageWrite {
                        page: both.clone(),
                        body: "cargo builds it and verification gates the close".to_string(),
                    },
                    PageWrite {
                        page: only_cargo.clone(),
                        body: "cargo is the build tool".to_string(),
                    },
                    PageWrite {
                        page: only_verify.clone(),
                        body: "verification runs before merge".to_string(),
                    },
                ],
                ..Default::default()
            })
            .unwrap();

        // Under the old implicit-AND this returned exactly one page (or zero);
        // every page carrying *either* term must now be found.
        let hits = store.search("cargo verification", 10).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.page.id.as_str()).collect();
        assert!(
            ids.contains(&only_cargo.id.as_str()) && ids.contains(&only_verify.id.as_str()),
            "disjunctive search must return single-term matches too; got {ids:?}"
        );

        // BM25 over a disjunctive match set still prefers the page carrying
        // both terms — this is why no separate AND-preference pass is needed.
        assert_eq!(
            hits[0].page.id, both.id,
            "the page matching both terms must rank first; got {ids:?}"
        );

        // A phrase the user quoted keeps adjacency: no page says "verification
        // cargo", so the phrase must match nothing even though both words are
        // individually present.
        assert!(
            store
                .search("\"verification cargo\"", 10)
                .unwrap()
                .is_empty(),
            "explicitly quoted phrases must not degrade into an OR of their words"
        );

        // ...while the same words unquoted do match, proving the phrase result
        // above is adjacency and not a tokenisation accident.
        assert!(!store.search("verification cargo", 10).unwrap().is_empty());
    }

    #[test]
    fn tombstoning_last_source_cascade_deletes_page_and_body() {
        let (_temp, store) = store();
        let solo = page(&store, "subsystem", "Legacy Module", &["src/legacy.rs"]);
        let shared = page(
            &store,
            "subsystem",
            "Core Module",
            &["src/legacy.rs", "src/core.rs"],
        );
        store
            .commit_ingest(&IngestBatch {
                pages: vec![
                    PageWrite {
                        page: solo.clone(),
                        body: "legacy notes".to_string(),
                    },
                    PageWrite {
                        page: shared.clone(),
                        body: "core notes".to_string(),
                    },
                ],
                sources: vec![
                    SourceOutcome {
                        file_path: "src/legacy.rs".to_string(),
                        blake3: "lll".to_string(),
                        size: 1,
                        status: SourceStatus::Ingested,
                        ingest_error: None,
                    },
                    SourceOutcome {
                        file_path: "src/core.rs".to_string(),
                        blake3: "ccc".to_string(),
                        size: 1,
                        status: SourceStatus::Ingested,
                        ingest_error: None,
                    },
                ],
                tombstones: vec![],
            })
            .unwrap();

        let solo_body = store.body_path(&solo.rel_path).unwrap();
        assert!(solo_body.exists());

        let report = store
            .commit_ingest(&IngestBatch {
                tombstones: vec!["src/legacy.rs".to_string()],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(report.sources_tombstoned, 1);
        assert_eq!(report.cascade_deleted_page_ids, vec![solo.id.clone()]);
        assert!(store.get_page(&solo.id).is_err());
        assert!(!solo_body.exists(), "orphan body must be unlinked");
        assert!(store.search("legacy", 10).unwrap().is_empty());

        // The multi-source page survives, minus the dead provenance entry.
        let survivor = store.get_page(&shared.id).unwrap();
        assert_eq!(survivor.sources, vec!["src/core.rs".to_string()]);
        assert!(store.get_source("src/legacy.rs").unwrap().is_none());
        assert!(store.get_source("src/core.rs").unwrap().is_some());
    }

    #[test]
    fn failed_source_is_recorded_and_reclassified_for_retry() {
        let (_temp, store) = store();
        store
            .commit_ingest(&IngestBatch {
                sources: vec![SourceOutcome {
                    file_path: "src/huge.rs".to_string(),
                    blake3: "hhh".to_string(),
                    size: 9,
                    status: SourceStatus::Failed,
                    ingest_error: Some("context window exceeded".to_string()),
                }],
                ..Default::default()
            })
            .unwrap();

        let row = store.get_source("src/huge.rs").unwrap().unwrap();
        assert_eq!(row.status, SourceStatus::Failed);
        assert_eq!(row.ingest_error.as_deref(), Some("context window exceeded"));

        let disk = vec![disk_row("src/huge.rs", "hhh", 9)];
        let classification = store.classify(&disk).unwrap();
        assert_eq!(classification.to_ingest.len(), 1);
        assert_eq!(classification.to_ingest[0].reason, IngestReason::Retry);
    }

    /// The strongest form of the crash-consistency claim: force a REAL failure
    /// inside `commit_ingest` at a point where an earlier page has already been
    /// inserted *and* FTS-indexed, and prove that nothing at all survives —
    /// rows, index entries and ledger rows roll back as one unit.
    ///
    /// Two pages with distinct rel_paths but the same `id` collide, so page 1
    /// writes its row + FTS entry and publishes its body, and page 2 then
    /// aborts the pass. Note the abort happens in the page phase, so the
    /// ledger assertion here is a guard against the ledger write ever moving
    /// outside the transaction — the *ordered* proof (failure AFTER the ledger
    /// write) is `failure_during_ledger_write_rolls_back_the_page_and_index_already_written`.
    #[test]
    fn mid_batch_failure_rolls_back_rows_index_and_ledger_together() {
        let (_temp, store) = store();

        // Seed durable prior state; it must survive the failed pass untouched.
        let existing = page(&store, "subsystem", "Existing", &["src/existing.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: existing.clone(),
                    body: "original body".to_string(),
                }],
                sources: vec![SourceOutcome {
                    file_path: "src/existing.rs".to_string(),
                    blake3: "eee".to_string(),
                    size: 1,
                    status: SourceStatus::Ingested,
                    ingest_error: None,
                }],
                tombstones: vec![],
            })
            .unwrap();

        let shared_id = store.generate_id().unwrap();
        let mut first = KnowledgePage::new(shared_id.clone(), "subsystem", "Alpha");
        first.sources = vec!["src/alpha.rs".to_string()];
        // Same id, different canonical path → UNIQUE(id) violation on insert #2.
        let mut second = KnowledgePage::new(shared_id, "subsystem", "Beta");
        second.sources = vec!["src/beta.rs".to_string()];

        let err = store.commit_ingest(&IngestBatch {
            pages: vec![
                PageWrite {
                    page: first.clone(),
                    body: "alpha body".to_string(),
                },
                PageWrite {
                    page: second,
                    body: "beta body".to_string(),
                },
            ],
            sources: vec![SourceOutcome {
                file_path: "src/alpha.rs".to_string(),
                blake3: "aaa".to_string(),
                size: 1,
                status: SourceStatus::Ingested,
                ingest_error: None,
            }],
            tombstones: vec![],
        });
        assert!(err.is_err(), "duplicate page id must abort the batch");

        // Nothing from the failed pass survives, in ANY of the three stores.
        assert!(store.get_page(&first.id).is_err(), "no page row leaked");
        assert!(
            store.get_source("src/alpha.rs").unwrap().is_none(),
            "the aborted pass must not record any ledger row"
        );
        assert!(
            store.search("alpha", 10).unwrap().is_empty(),
            "FTS entry for the rolled-back page must not survive"
        );

        // Prior state is intact and the index still agrees with the rows.
        assert_eq!(store.list_pages().unwrap().len(), 1);
        assert_eq!(
            store.read_body(&existing.rel_path).unwrap(),
            "original body"
        );
        assert_eq!(store.search("original", 10).unwrap().len(), 1);
        assert_eq!(
            store.get_source("src/existing.rs").unwrap().unwrap().status,
            SourceStatus::Ingested
        );

        let conn = store.lock();
        let fts_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM knowledge_pages_fts", [], |r| r.get(0))
            .unwrap();
        let page_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM knowledge_pages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            (fts_rows, page_rows),
            (1, 1),
            "index and row count must match after a mid-batch abort"
        );
    }

    /// Validation runs before anything is staged or opened, so a batch with one
    /// bad page never touches the disk or the database at all.
    ///
    /// (The crash-consistency claim itself is proven by the fault-injection
    /// tests above, which force real failures inside `commit_ingest` rather
    /// than hand-rolling a transaction that the store never executes.)
    #[test]
    fn validation_rejects_a_batch_before_touching_disk_or_db() {
        let (_temp, store) = store();

        let good = page(&store, "subsystem", "Good Page", &["src/good.rs"]);
        let mut bad = page(&store, "subsystem", "Bad Page", &["src/bad.rs"]);
        bad.rel_path = "../escape.md".to_string();

        let err = store.commit_ingest(&IngestBatch {
            pages: vec![
                PageWrite {
                    page: good.clone(),
                    body: "good body".to_string(),
                },
                PageWrite {
                    page: bad,
                    body: "bad body".to_string(),
                },
            ],
            sources: vec![SourceOutcome {
                file_path: "src/good.rs".to_string(),
                blake3: "ggg".to_string(),
                size: 1,
                status: SourceStatus::Ingested,
                ingest_error: None,
            }],
            tombstones: vec![],
        });
        assert!(err.is_err());
        assert_eq!(counts(&store), (0, 0, 0));
        assert!(
            !store.body_path(&good.rel_path).unwrap().exists(),
            "the valid page's body must not be staged into place either"
        );

        // An empty page id is rejected the same way.
        let mut anonymous = page(&store, "subsystem", "Anonymous", &[]);
        anonymous.id = "  ".to_string();
        assert!(
            store
                .commit_ingest(&IngestBatch {
                    pages: vec![PageWrite {
                        page: anonymous,
                        body: "x".to_string()
                    }],
                    ..Default::default()
                })
                .is_err()
        );
        assert_eq!(counts(&store), (0, 0, 0));
    }

    #[test]
    fn every_indexed_page_has_a_row_and_a_body() {
        // Invariant sweep after a mixed pass: FTS rowids ⊆ page rowids, and
        // every page row has a body file.
        let (_temp, store) = store();
        let mut pages = Vec::new();
        for i in 0..5 {
            let p = page(&store, "subsystem", &format!("Module {i}"), &["src/x.rs"]);
            pages.push(PageWrite {
                page: p,
                body: format!("body number {i}"),
            });
        }
        store
            .commit_ingest(&IngestBatch {
                pages,
                ..Default::default()
            })
            .unwrap();
        store
            .delete_page(&store.list_pages().unwrap()[0].id)
            .unwrap();

        let conn = store.lock();
        let orphan_fts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM knowledge_pages_fts
                 WHERE rowid NOT IN (SELECT row_id FROM knowledge_pages)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_fts, 0);
        drop(conn);

        for p in store.list_pages().unwrap() {
            assert!(store.body_path(&p.rel_path).unwrap().exists());
        }
        assert_eq!(store.list_pages().unwrap().len(), 4);
    }

    #[test]
    fn blake3_helper_is_stable_and_matches_file_hash() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("f.txt");
        std::fs::write(&path, b"hello knowledge").unwrap();
        let (hash, size) = hash_source_file(&path).unwrap();
        assert_eq!(size, 15);
        assert_eq!(hash, blake3_hex(b"hello knowledge"));
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, blake3_hex(b"hello knowledg3"));
    }

    #[test]
    fn locking_after_creation_protects_the_page_and_survives_reingest() {
        let (_temp, store) = store();
        let p = page(&store, "workflow", "Deploy Steps", &["docs/deploy.md"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "auto-distilled".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        // The user edits and locks the page after the fact.
        store.set_locked(&p.id, true).unwrap();
        std::fs::write(store.body_path(&p.rel_path).unwrap(), "human edit").unwrap();
        assert!(store.get_page(&p.id).unwrap().locked);

        let report = store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: page(&store, "workflow", "Deploy Steps", &["docs/deploy.md"]),
                    body: "llm overwrite".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.pages_written, 0);
        assert_eq!(store.read_body(&p.rel_path).unwrap(), "human edit");

        // Unlocking hands control back to distillation.
        store.set_locked(&p.id, false).unwrap();
        let report = store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: page(&store, "workflow", "Deploy Steps", &["docs/deploy.md"]),
                    body: "llm overwrite".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.pages_written, 1);
        assert_eq!(store.read_body(&p.rel_path).unwrap(), "llm overwrite");

        assert!(store.set_locked("cas-knmissing", true).is_err());
    }

    #[test]
    fn mark_all_pending_embedding_rearms_every_page_without_touching_updated_at() {
        let (_temp, store) = store();
        let mut ids = Vec::new();
        for title in ["Alpha", "Beta"] {
            let p = page(&store, "subsystem", title, &["src/lib.rs"]);
            ids.push(p.id.clone());
            store
                .commit_ingest(&IngestBatch {
                    pages: vec![PageWrite {
                        page: p,
                        body: "body".to_string(),
                    }],
                    ..Default::default()
                })
                .unwrap();
        }
        for id in &ids {
            store.mark_embedded(id).unwrap();
        }
        assert!(store.list_pending_embedding(10).unwrap().is_empty());
        let before: Vec<_> = ids
            .iter()
            .map(|id| store.get_page(id).unwrap().updated_at)
            .collect();

        // This is what an embedding-model change triggers: every cached
        // vector is now from the wrong space, so every page must be redone.
        assert_eq!(store.mark_all_pending_embedding().unwrap(), 2);
        assert_eq!(store.list_pending_embedding(10).unwrap().len(), 2);

        let after: Vec<_> = ids
            .iter()
            .map(|id| store.get_page(id).unwrap().updated_at)
            .collect();
        assert_eq!(
            before, after,
            "re-arming embeddings is an internal cache concern; bumping \
             updated_at would make every page look edited to sync conflict \
             resolution and re-push the whole wiki"
        );

        // Idempotent: nothing left to re-arm on a second call.
        assert_eq!(store.mark_all_pending_embedding().unwrap(), 0);
    }

    #[test]
    fn mark_embedded_clears_the_pending_flag() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Embeddings", &["src/embed.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.list_pending_embedding(10).unwrap().len(), 1);

        store.mark_embedded(&p.id).unwrap();
        assert!(store.list_pending_embedding(10).unwrap().is_empty());
        assert!(!store.get_page(&p.id).unwrap().pending_embedding);

        // A re-distillation marks it dirty again.
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: page(&store, "subsystem", "Embeddings", &["src/embed.rs"]),
                    body: "new body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.list_pending_embedding(10).unwrap().len(), 1);

        assert!(store.mark_embedded("cas-knmissing").is_err());
    }

    // ── Fault injection: force a REAL failure after the ledger write ────

    /// Install a trigger that aborts the next `knowledge_sources` insert, so a
    /// batch fails in phase 2 (ledger) *after* phase 1 (page row + FTS + body)
    /// has already run. This is the only way to prove the phases share one
    /// transaction rather than merely appearing to.
    fn arm_ledger_bomb(store: &SqliteKnowledgeStore) {
        let conn = store.lock();
        conn.execute_batch(
            "CREATE TEMP TRIGGER knowledge_bomb BEFORE INSERT ON knowledge_sources
             BEGIN SELECT RAISE(ABORT, 'bomb'); END",
        )
        .unwrap();
    }

    fn disarm_ledger_bomb(store: &SqliteKnowledgeStore) {
        let conn = store.lock();
        conn.execute_batch("DROP TRIGGER IF EXISTS temp.knowledge_bomb")
            .unwrap();
    }

    fn counts(store: &SqliteKnowledgeStore) -> (i64, i64, i64) {
        let conn = store.lock();
        let pages = conn
            .query_row("SELECT COUNT(*) FROM knowledge_pages", [], |r| r.get(0))
            .unwrap();
        let fts = conn
            .query_row("SELECT COUNT(*) FROM knowledge_pages_fts", [], |r| r.get(0))
            .unwrap();
        let sources = conn
            .query_row("SELECT COUNT(*) FROM knowledge_sources", [], |r| r.get(0))
            .unwrap();
        (pages, fts, sources)
    }

    /// AC3, proven the hard way: the page row and its FTS entry are written
    /// FIRST, then the ledger write blows up. Everything must roll back —
    /// rows, index, ledger and the body file on disk.
    #[test]
    fn failure_during_ledger_write_rolls_back_the_page_and_index_already_written() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Doomed", &["src/doomed.rs"]);
        let body_path = store.body_path(&p.rel_path).unwrap();

        arm_ledger_bomb(&store);
        let err = store.commit_ingest(&IngestBatch {
            pages: vec![PageWrite {
                page: p.clone(),
                body: "doomed body".to_string(),
            }],
            sources: vec![SourceOutcome {
                file_path: "src/doomed.rs".to_string(),
                blake3: "ddd".to_string(),
                size: 1,
                status: SourceStatus::Ingested,
                ingest_error: None,
            }],
            tombstones: vec![],
        });
        assert!(err.is_err(), "the ledger write must fail the pass");
        disarm_ledger_bomb(&store);

        assert_eq!(
            counts(&store),
            (0, 0, 0),
            "page row, FTS entry and ledger row must roll back together"
        );
        assert!(
            !body_path.exists(),
            "the published body must be rolled back too, not left ahead of the DB"
        );

        // The store still works, and a retry succeeds cleanly.
        let report = store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "doomed body".to_string(),
                }],
                sources: vec![SourceOutcome {
                    file_path: "src/doomed.rs".to_string(),
                    blake3: "ddd".to_string(),
                    size: 1,
                    status: SourceStatus::Ingested,
                    ingest_error: None,
                }],
                tombstones: vec![],
            })
            .unwrap();
        assert_eq!(report.pages_written, 1);
        assert_eq!(counts(&store), (1, 1, 1));
    }

    /// The other direction of the same hole: a failed pass must not leave an
    /// EXISTING page's body file updated while its row rolls back.
    #[test]
    fn failed_pass_leaves_an_existing_body_byte_for_byte_unchanged() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Stable", &["src/stable.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "original body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        arm_ledger_bomb(&store);
        let err = store.commit_ingest(&IngestBatch {
            pages: vec![PageWrite {
                page: page(&store, "subsystem", "Stable", &["src/stable.rs"]),
                body: "replacement body".to_string(),
            }],
            sources: vec![SourceOutcome {
                file_path: "src/stable.rs".to_string(),
                blake3: "sss".to_string(),
                size: 1,
                status: SourceStatus::Ingested,
                ingest_error: None,
            }],
            tombstones: vec![],
        });
        assert!(err.is_err());
        disarm_ledger_bomb(&store);

        assert_eq!(
            store.read_body(&p.rel_path).unwrap(),
            "original body",
            "disk must not run ahead of the database on an aborted pass"
        );
        assert_eq!(store.search("original", 10).unwrap().len(), 1);
        assert!(store.search("replacement", 10).unwrap().is_empty());
        // No stray temp or backup files left behind.
        let leftovers: Vec<_> =
            std::fs::read_dir(store.body_path(&p.rel_path).unwrap().parent().unwrap())
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.contains("tmp") || n.contains("bak"))
                .collect();
        assert!(leftovers.is_empty(), "staging leftovers: {leftovers:?}");
    }

    /// A locked page's hand-written body must survive a distillation pass that
    /// targets it — including when the lock is created earlier in the SAME
    /// batch, which needs no race at all.
    #[test]
    fn locked_page_body_is_not_clobbered_even_within_one_batch() {
        let (_temp, store) = store();
        let mut human = page(&store, "workflow", "Release", &["docs/release.md"]);
        human.locked = true;
        let rel_path = human.rel_path.clone();

        let report = store
            .commit_ingest(&IngestBatch {
                pages: vec![
                    PageWrite {
                        page: human,
                        body: "HUMAN TRUTH".to_string(),
                    },
                    // Same canonical path, arrives later in the same batch.
                    PageWrite {
                        page: page(&store, "workflow", "Release", &["docs/release.md"]),
                        body: "LLM GUESS".to_string(),
                    },
                ],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(report.pages_written, 1);
        assert_eq!(report.locked_skipped_rel_paths, vec![rel_path.clone()]);
        assert_eq!(
            store.read_body(&rel_path).unwrap(),
            "HUMAN TRUTH",
            "the locked page's body must never be overwritten on disk"
        );
        assert_eq!(store.search("HUMAN", 10).unwrap().len(), 1);
        assert!(store.search("GUESS", 10).unwrap().is_empty());
    }

    // ── Locked-bit completeness ─────────────────────────────────────────

    #[test]
    fn distillation_cannot_lock_a_page_the_user_did_not_lock() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Open Page", &["src/open.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "v1".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        let mut sneaky = page(&store, "subsystem", "Open Page", &["src/open.rs"]);
        sneaky.locked = true; // LLM-produced page asking to be frozen
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: sneaky,
                    body: "v2".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        assert!(
            !store.get_page(&p.id).unwrap().locked,
            "commit_ingest must never set locked on an existing page"
        );
    }

    #[test]
    fn tombstone_cascade_spares_a_locked_page_and_its_body() {
        let (_temp, store) = store();
        let mut p = page(&store, "workflow", "Curated", &["src/gone.rs"]);
        p.locked = true;
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "curated by hand".to_string(),
                }],
                sources: vec![SourceOutcome {
                    file_path: "src/gone.rs".to_string(),
                    blake3: "ggg".to_string(),
                    size: 1,
                    status: SourceStatus::Ingested,
                    ingest_error: None,
                }],
                tombstones: vec![],
            })
            .unwrap();

        let report = store
            .commit_ingest(&IngestBatch {
                tombstones: vec!["src/gone.rs".to_string()],
                ..Default::default()
            })
            .unwrap();

        assert!(
            report.cascade_deleted_page_ids.is_empty(),
            "a locked page must survive losing its last source"
        );
        let survivor = store.get_page(&p.id).unwrap();
        assert!(survivor.sources.is_empty());
        assert_eq!(store.read_body(&p.rel_path).unwrap(), "curated by hand");
    }

    // ── Provenance matching ─────────────────────────────────────────────

    #[test]
    fn provenance_matching_handles_substrings_and_json_escaped_paths() {
        let (_temp, store) = store();
        let nested = page(&store, "subsystem", "Vendored", &["vendor/src/legacy.rs"]);
        let quoted_path = r#"src/we"ird.rs"#;
        let quoted = page(&store, "subsystem", "Quoted", &[quoted_path]);
        let plain = page(&store, "subsystem", "Plain", &["src/legacy.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![
                    PageWrite {
                        page: nested.clone(),
                        body: "vendored".to_string(),
                    },
                    PageWrite {
                        page: quoted.clone(),
                        body: "quoted".to_string(),
                    },
                    PageWrite {
                        page: plain.clone(),
                        body: "plain".to_string(),
                    },
                ],
                sources: vec![
                    SourceOutcome {
                        file_path: quoted_path.to_string(),
                        blake3: "qqq".to_string(),
                        size: 1,
                        status: SourceStatus::Ingested,
                        ingest_error: None,
                    },
                    SourceOutcome {
                        file_path: "src/legacy.rs".to_string(),
                        blake3: "lll".to_string(),
                        size: 1,
                        status: SourceStatus::Ingested,
                        ingest_error: None,
                    },
                ],
                tombstones: vec![],
            })
            .unwrap();

        // Exact provenance lookup, not substring.
        let hits = store.pages_for_source("src/legacy.rs").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, plain.id);
        // A path needing JSON escaping is still found.
        assert_eq!(
            store.pages_for_source(quoted_path).unwrap()[0].id,
            quoted.id
        );

        // Tombstoning the escaped path cascades correctly.
        let report = store
            .commit_ingest(&IngestBatch {
                tombstones: vec![quoted_path.to_string()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.cascade_deleted_page_ids, vec![quoted.id.clone()]);

        // Tombstoning src/legacy.rs must not touch vendor/src/legacy.rs's page.
        let report = store
            .commit_ingest(&IngestBatch {
                tombstones: vec!["src/legacy.rs".to_string()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.cascade_deleted_page_ids, vec![plain.id.clone()]);
        let survivor = store.get_page(&nested.id).unwrap();
        assert_eq!(survivor.sources, vec!["vendor/src/legacy.rs".to_string()]);
    }

    // ── Misc guards ─────────────────────────────────────────────────────

    #[test]
    fn reusing_a_page_id_for_a_different_path_fails_with_a_clear_error() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "First", &["src/a.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "first".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        let mut clash = KnowledgePage::new(p.id.clone(), "subsystem", "Second");
        clash.sources = vec!["src/b.rs".to_string()];
        let err = store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: clash,
                    body: "second".to_string(),
                }],
                ..Default::default()
            })
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&p.id) && msg.contains("already belongs to"),
            "expected a clear id-collision error, got: {msg}"
        );
        // Nothing leaked from the rejected batch.
        assert_eq!(store.list_pages().unwrap().len(), 1);
        assert!(
            store
                .get_page_by_rel_path("subsystem/second.md")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn delete_page_removes_the_body_and_reports_a_missing_page() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Doomed", &["src/x.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();
        let body_path = store.body_path(&p.rel_path).unwrap();
        assert!(body_path.exists());

        store.delete_page(&p.id).unwrap();
        assert!(!body_path.exists(), "delete_page must unlink the body");
        assert!(store.search("body", 10).unwrap().is_empty());
        assert!(store.delete_page("cas-knmissing").is_err());
    }

    #[test]
    fn local_delete_creates_a_pending_tombstone_that_survives_the_page() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Doomed", &["src/x.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();

        store.delete_page(&p.id).unwrap();
        assert!(store.get_page(&p.id).is_err());
        let pending = store.list_pending_page_tombstones().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, p.id);
        assert!(pending[0].locally_authored);

        store.mark_page_tombstones_pushed(&[p.id.clone()]).unwrap();
        assert!(store.list_pending_page_tombstones().unwrap().is_empty());
        assert!(
            store.is_page_tombstoned(&p.id).unwrap(),
            "delivery must not discard the durable no-resurrection guard"
        );
    }

    #[test]
    fn cascade_delete_creates_a_pending_tombstone() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Only Source", &["src/gone.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "body".to_string(),
                }],
                sources: vec![SourceOutcome {
                    file_path: "src/gone.rs".to_string(),
                    blake3: "abc".to_string(),
                    size: 1,
                    status: SourceStatus::Ingested,
                    ingest_error: None,
                }],
                tombstones: Vec::new(),
            })
            .unwrap();

        let report = store
            .commit_ingest(&IngestBatch {
                tombstones: vec!["src/gone.rs".to_string()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.cascade_deleted_page_ids, vec![p.id.clone()]);
        assert_eq!(
            store
                .list_pending_page_tombstones()
                .unwrap()
                .into_iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![p.id.clone()]
        );
    }

    #[test]
    fn remote_tombstone_spares_a_locked_page_but_blocks_its_stale_record() {
        let (_temp, store) = store();
        let p = page(&store, "subsystem", "Human Page", &["src/x.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page: p.clone(),
                    body: "human body".to_string(),
                }],
                ..Default::default()
            })
            .unwrap();
        store.set_locked(&p.id, true).unwrap();

        assert_eq!(
            store
                .apply_remote_page_tombstone(&p.id, Utc::now())
                .unwrap(),
            TombstoneApplyOutcome::LockedPreserved
        );
        assert_eq!(store.read_body(&p.rel_path).unwrap(), "human body");
        assert!(store.is_page_tombstoned(&p.id).unwrap());

        let stale = PageWrite {
            page: p.clone(),
            body: "stale remote body".to_string(),
        };
        let report = store
            .commit_ingest(&IngestBatch {
                pages: vec![stale],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.tombstoned_skipped_page_ids, vec![p.id]);
        assert_eq!(store.read_body(&p.rel_path).unwrap(), "human body");
    }

    #[test]
    fn search_ranks_by_relevance_and_honours_the_limit() {
        let (_temp, store) = store();
        let strong = page(&store, "subsystem", "Strong", &["a.rs"]);
        let weak = page(&store, "subsystem", "Weak", &["b.rs"]);
        store
            .commit_ingest(&IngestBatch {
                pages: vec![
                    PageWrite {
                        page: strong.clone(),
                        body: "widget widget widget widget widget".to_string(),
                    },
                    PageWrite {
                        page: weak.clone(),
                        body: "widget appears once among many other unrelated words".to_string(),
                    },
                ],
                ..Default::default()
            })
            .unwrap();

        let hits = store.search("widget", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].page.id, strong.id,
            "the denser match must rank first"
        );
        assert!(
            hits[0].score <= hits[1].score,
            "bm25 scores must be ascending (lower is better): {:?}",
            hits.iter().map(|h| h.score).collect::<Vec<_>>()
        );
        assert_ne!(hits[0].score, 0.0, "score must be a real bm25 value");
        assert_eq!(store.search("widget", 1).unwrap().len(), 1, "LIMIT applies");
    }

    #[test]
    fn generate_id_recovers_from_a_stale_sequence() {
        let (_temp, store) = store();
        // Simulate a restored backup: rows exist far past the sequence counter.
        {
            let conn = store.lock();
            for n in [1i64, 2, 3, 42] {
                conn.execute(
                    "INSERT INTO knowledge_pages
                        (id, page_type, title, rel_path, snippet, locked, sources_json,
                         created_at, updated_at, pending_embedding)
                     VALUES (?1, 'subsystem', 'T', ?2, '', 0, '[]',
                             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1)",
                    params![format!("cas-kn{n:03}"), format!("subsystem/t{n}.md")],
                )
                .unwrap();
            }
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS id_sequences (
                    name TEXT PRIMARY KEY,
                    next_val INTEGER NOT NULL DEFAULT 1
                )",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO id_sequences (name, next_val) VALUES ('knowledge_page', 0)
                 ON CONFLICT(name) DO UPDATE SET next_val = 0",
                [],
            )
            .unwrap();
        }

        let id = store.generate_id().unwrap();
        assert_eq!(
            id, "cas-kn043",
            "the sequence must jump past the highest id in use"
        );
        assert!(store.get_page(&id).is_err(), "the new id must be free");
    }

    #[test]
    fn every_page_row_has_an_index_entry_and_vice_versa() {
        let (_temp, store) = store();
        let pages: Vec<_> = (0..4)
            .map(|i| PageWrite {
                page: page(&store, "subsystem", &format!("Mod {i}"), &["src/x.rs"]),
                body: format!("body {i}"),
            })
            .collect();
        store
            .commit_ingest(&IngestBatch {
                pages,
                ..Default::default()
            })
            .unwrap();
        store
            .delete_page(&store.list_pages().unwrap()[0].id)
            .unwrap();

        let conn = store.lock();
        let unindexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM knowledge_pages
                 WHERE row_id NOT IN (SELECT rowid FROM knowledge_pages_fts)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let orphaned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM knowledge_pages_fts
                 WHERE rowid NOT IN (SELECT row_id FROM knowledge_pages)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            (unindexed, orphaned),
            (0, 0),
            "rows and index entries must correspond exactly, both directions"
        );
    }

    #[test]
    fn empty_batch_is_a_no_op() {
        let (_temp, store) = store();
        let report = store.commit_ingest(&IngestBatch::default()).unwrap();
        assert_eq!(report, IngestReport::default());
        assert_eq!(counts(&store), (0, 0, 0));
    }

    // ── Parsing / fallback behaviour ────────────────────────────────────

    #[test]
    fn parse_helpers_have_defined_fallbacks() {
        // Timestamps: RFC3339, the legacy space-separated form, then now().
        let rfc = SqliteKnowledgeStore::parse_datetime("2026-01-02T03:04:05Z");
        assert_eq!(rfc.to_rfc3339(), "2026-01-02T03:04:05+00:00");
        let legacy = SqliteKnowledgeStore::parse_datetime("2026-01-02 03:04:05");
        assert_eq!(legacy.to_rfc3339(), "2026-01-02T03:04:05+00:00");
        // Garbage falls back to "now" rather than failing the whole read.
        let fallback = SqliteKnowledgeStore::parse_datetime("not a date");
        assert!((Utc::now() - fallback).num_seconds().abs() < 5);

        // Provenance: empty and malformed JSON both degrade to "no sources"
        // (which is safe: the cascade only deletes on an exact source match).
        assert!(SqliteKnowledgeStore::parse_sources("").is_empty());
        assert!(SqliteKnowledgeStore::parse_sources("{oops").is_empty());
        assert_eq!(
            SqliteKnowledgeStore::parse_sources(r#"["a.rs","b.rs"]"#),
            vec!["a.rs".to_string(), "b.rs".to_string()]
        );

        // Status parsing is case/whitespace tolerant and rejects the unknown.
        assert_eq!(
            " INGESTED ".parse::<SourceStatus>().unwrap(),
            SourceStatus::Ingested
        );
        assert!("bogus".parse::<SourceStatus>().is_err());

        // json_fragment escapes exactly the way serde_json stores it.
        assert_eq!(json_fragment("plain.rs"), "plain.rs");
        assert_eq!(json_fragment(r#"we"ird.rs"#), r#"we\"ird.rs"#);
    }

    #[test]
    fn read_body_reports_a_missing_file_as_not_found() {
        let (_temp, store) = store();
        let err = store.read_body("subsystem/absent.md").unwrap_err();
        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
        assert!(store.read_body("../escape.md").is_err());
    }

    #[test]
    fn hash_source_file_propagates_a_missing_path() {
        let temp = TempDir::new().unwrap();
        assert!(hash_source_file(&temp.path().join("nope.txt")).is_err());
    }

    #[test]
    fn generated_ids_are_unique() {
        let (_temp, store) = store();
        let mut seen = HashSet::new();
        for _ in 0..50 {
            assert!(seen.insert(store.generate_id().unwrap()));
        }
    }
}
