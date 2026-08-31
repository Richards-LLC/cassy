//! Retrieval parity harness (cas-90fd, phase M4 of the memory → knowledge
//! migration).
//!
//! # Why this exists
//!
//! The memory → knowledge migration (cas-b129) rewrites where memories live
//! and how they are indexed. "Zero loss" is only checkable if we know what the
//! *current* system returns, so this harness has two modes:
//!
//! * **capture** — run a committed query set against today's retrieval
//!   surfaces and write a baseline fixture. Must be run *before* the
//!   migration; it needs nothing but the current system.
//! * **replay** — run the same query set against the post-migration system and
//!   diff it against the baseline, reporting per-query regressions and exiting
//!   non-zero if any are found.
//!
//! # The fingerprint decision
//!
//! Hits are matched on a **normalized-content fingerprint**, not on entry id.
//! The migration is expected to re-key entries (legacy `p-YYYY-MM-DD-NNN` ids
//! do not survive into the knowledge store), so an id-keyed baseline would
//! report every single hit as lost and tell us nothing. Ids are still recorded
//! for diagnostics, but the parity question this harness answers is *"is the
//! same knowledge still retrievable at the same rank?"* — which is a question
//! about content, not about primary keys.
//!
//! # Read-only guarantee
//!
//! Neither mode may write to the legacy store or the knowledge store.
//!
//! * Store reads go through [`store_ro::ReadOnlyMemoryDb`], a connection
//!   opened `SQLITE_OPEN_READ_ONLY` — writes fail at the SQLite layer rather
//!   than relying on the harness not to attempt them. In particular this
//!   avoids [`cas_store::SqliteStore::open`], which takes a read-write
//!   connection from the shared pool.
//! * The search channel refuses to call `SearchIndex::open` unless a
//!   compatible index already exists, because that constructor
//!   *deletes and recreates* the index directory on a schema-field-count
//!   mismatch and *creates* it when absent. A parity run must never be the
//!   thing that rebuilds the index it is measuring; when the index is missing
//!   or stale the channel reports [`ChannelStatus::Unavailable`] instead.

pub mod channels;
pub mod diff;
pub mod queryset;
pub mod store_ro;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use channels::{ChannelStatus, IndexHandle, RunEnv, SESSION_MERGE_LIMIT, run_case};
pub use diff::{Regression, RegressionKind, Report, diff_baseline};
pub use queryset::{Channel, QueryCase, QuerySet};

/// Baseline format version. Bump when the on-disk shape changes so that a
/// replay against an older fixture fails loudly instead of silently
/// mis-comparing.
pub const BASELINE_VERSION: u32 = 1;

/// Default number of positions a hit may slip before it counts as a
/// regression. Ranking is not bit-stable across index rebuilds, so a tiny
/// amount of drift is expected; losing a hit entirely never is.
pub const DEFAULT_RANK_TOLERANCE: usize = 3;

/// Default result depth per query.
pub const DEFAULT_LIMIT: usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum ParityError {
    #[error("memory store unavailable: {0}")]
    StoreUnavailable(String),
    #[error("sql error: {0}")]
    Sql(String),
    #[error("query set error: {0}")]
    QuerySet(String),
    #[error("baseline error: {0}")]
    Baseline(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "query set does not cover the corpus: {0}. Add cases to the query set, \
         or pass --allow-uncovered to capture a knowingly partial baseline."
    )]
    Coverage(String),
}

/// A single retrieved memory, as recorded in a baseline or a replay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hit {
    /// 0-based position in the result list.
    pub rank: usize,
    /// Store id at capture time. Diagnostics only — see the module docs on why
    /// this is not the match key.
    pub id: String,
    /// Normalized-content fingerprint. This *is* the match key.
    pub fp: String,
    /// Truncated title/preview, so a human reading a regression report can
    /// tell what was lost without going back to the store.
    pub label: String,
    pub entry_type: String,
    pub tier: String,
}

/// Result of running one query case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub id: String,
    pub channel: Channel,
    pub query: String,
    pub status: ChannelStatus,
    pub hits: Vec<Hit>,
}

/// Shape of the corpus at capture time, recorded so that an out-of-family
/// replay (different machine, emptied store) is obvious in the report.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CorpusStats {
    pub active_entries: usize,
    pub entry_types: Vec<String>,
    pub tiers: Vec<String>,
}

/// A committed snapshot of what the current system retrieves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,
    pub machine: String,
    pub captured_at: String,
    pub cas_dir: String,
    pub corpus: CorpusStats,
    pub results: Vec<QueryResult>,
}

/// Everything a capture or replay run needs to reach the stores.
pub struct ParityContext {
    pub cas_dir: PathBuf,
    /// Tantivy index directory; defaults to the schema-versioned resolver
    /// below `<cas_dir>/index/`.
    pub index_dir: PathBuf,
    /// Global memory store directory (the host's `~/.cas`, see
    /// [`crate::cli::retrieval_parity::resolve_global_cas_dir`]), when it holds
    /// a readable database. Only the `session_merge` channel reads it,
    /// mirroring `merge_entries`.
    pub global_cas_dir: Option<PathBuf>,
    /// Why a *requested* global store could not be used. `None` means either
    /// "no global store was asked for" (project-only run) or "the one asked for
    /// is usable" — the two are distinguished by `global_cas_dir`.
    ///
    /// A requested-but-unusable global store must never degrade into a silent
    /// project-only run: it is carried here and surfaced as
    /// [`ChannelStatus::Unavailable`] on `session_merge`, for the same reason a
    /// missing search index is (see [`channels::ChannelStatus`]).
    pub global_unavailable: Option<String>,
}

impl ParityContext {
    /// Project-only context. Use [`ParityContext::with_global`] to include the
    /// global store in the SessionStart merge channel.
    pub fn new(cas_dir: &Path) -> Self {
        Self {
            cas_dir: cas_dir.to_path_buf(),
            index_dir: crate::hybrid_search::tantivy_index_dir(cas_dir),
            global_cas_dir: None,
            global_unavailable: None,
        }
    }

    /// Attach a global store.
    ///
    /// A path that holds no `cas.db` is **not** silently dropped — dropping it
    /// is what let every parity run on this host measure the project store
    /// alone while reporting green (cas-96ae). It is recorded as an
    /// unavailability reason instead, which makes `session_merge` report
    /// [`ChannelStatus::Unavailable`] rather than an empty-but-healthy merge.
    ///
    /// `None` is an explicit project-only run and stays quiet.
    pub fn with_global(mut self, global_cas_dir: Option<PathBuf>) -> Self {
        match global_cas_dir {
            Some(dir) if dir.join("cas.db").exists() => {
                self.global_cas_dir = Some(dir);
                self.global_unavailable = None;
            }
            Some(dir) => {
                self.global_cas_dir = None;
                self.global_unavailable = Some(format!(
                    "global store requested at {} but there is no cas.db there; \
                     the SessionStart merge cannot be reproduced and this run \
                     measures the project store only",
                    dir.display()
                ));
            }
            None => {
                self.global_cas_dir = None;
                self.global_unavailable = None;
            }
        }
        self
    }
}

/// Fingerprint a memory's content.
///
/// Normalization is deliberately aggressive — lowercase, whitespace collapsed,
/// ends trimmed — so that reformatting during migration (re-wrapping, heading
/// reflow, trailing-newline changes) does not read as knowledge loss. Anything
/// coarser would start colliding distinct short memories; anything finer would
/// produce false regressions on cosmetic rewrites.
pub fn fingerprint(content: &str) -> String {
    let normalized: String = content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    // 16 hex chars = 64 bits. At corpus sizes in the low millions the
    // birthday-collision probability is still under 1e-7.
    hex16(&digest)
}

fn hex16(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// Short human label for a hit.
pub fn label_for(title: Option<&str>, content: &str) -> String {
    let raw = match title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => content.split_whitespace().collect::<Vec<_>>().join(" "),
    };
    let mut out: String = raw.chars().take(80).collect();
    if raw.chars().count() > 80 {
        out.push('…');
    }
    out
}

/// This machine's identifier, used to name and validate baseline fixtures.
pub fn machine_id() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|h| {
            h.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect()
        })
        .unwrap_or_else(|| "unknown-machine".to_string())
}

/// Run every case in `set` against the current system.
pub fn run_query_set(ctx: &ParityContext, set: &QuerySet) -> Result<Vec<QueryResult>, ParityError> {
    let db = store_ro::ReadOnlyMemoryDb::open(&ctx.cas_dir)?;
    let global = match &ctx.global_cas_dir {
        Some(dir) => Some(store_ro::ReadOnlyMemoryDb::open(dir)?),
        None => None,
    };
    let index = channels::open_index_if_compatible(&ctx.index_dir);
    let env = channels::RunEnv {
        project: &db,
        global: global.as_ref(),
        global_unavailable: ctx.global_unavailable.as_deref(),
        index: &index,
        excluded: set.excluded_fingerprints(),
    };
    let mut out = Vec::with_capacity(set.query.len());
    for case in &set.query {
        out.push(run_case(&env, case, set)?);
    }
    Ok(out)
}

/// Capture mode: run the query set and build a baseline.
///
/// `allow_uncovered` relaxes the corpus-coverage check; without it, a query
/// set that misses an entry type or tier present in the store is an error,
/// because a baseline with blind spots silently blesses whatever the migration
/// does in them.
pub fn capture(
    ctx: &ParityContext,
    set: &QuerySet,
    now_rfc3339: String,
    allow_uncovered: bool,
) -> Result<Baseline, ParityError> {
    let db = store_ro::ReadOnlyMemoryDb::open(&ctx.cas_dir)?;
    let corpus = CorpusStats {
        active_entries: db.active_count()?,
        entry_types: db.distinct_types()?,
        tiers: db.distinct_tiers()?,
    };
    drop(db);

    let gaps = set.coverage_gaps(&corpus);
    if !gaps.is_empty() && !allow_uncovered {
        return Err(ParityError::Coverage(gaps.join("; ")));
    }

    let results = run_query_set(ctx, set)?;
    Ok(Baseline {
        version: BASELINE_VERSION,
        machine: machine_id(),
        captured_at: now_rfc3339,
        cas_dir: ctx.cas_dir.display().to_string(),
        corpus,
        results,
    })
}

/// Replay mode: run the query set again and diff against `baseline`.
pub fn replay(
    ctx: &ParityContext,
    set: &QuerySet,
    baseline: &Baseline,
    rank_tolerance: usize,
) -> Result<Report, ParityError> {
    if baseline.version != BASELINE_VERSION {
        return Err(ParityError::Baseline(format!(
            "baseline is version {} but this harness speaks version {}; \
             re-capture rather than comparing across formats",
            baseline.version, BASELINE_VERSION
        )));
    }
    let results = run_query_set(ctx, set)?;
    let by_id: std::collections::HashMap<&str, &QueryCase> =
        set.query.iter().map(|c| (c.id.as_str(), c)).collect();
    Ok(diff::diff_baseline_with(
        baseline,
        &results,
        rank_tolerance,
        |case_id| match by_id.get(case_id) {
            Some(case) => set.tolerance_for(case, Some(rank_tolerance)),
            // A baseline case the query set no longer defines is reported as
            // CaseMissing regardless; the tolerance is immaterial.
            None => rank_tolerance,
        },
    ))
}

/// Read a baseline fixture from disk.
pub fn load_baseline(path: &Path) -> Result<Baseline, ParityError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        ParityError::Baseline(format!("cannot read baseline {}: {e}", path.display()))
    })?;
    serde_json::from_str(&raw)
        .map_err(|e| ParityError::Baseline(format!("malformed baseline {}: {e}", path.display())))
}

/// Write a baseline fixture to disk, pretty-printed so it diffs readably in
/// review.
pub fn save_baseline(path: &Path, baseline: &Baseline) -> Result<(), ParityError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(baseline)
        .map_err(|e| ParityError::Baseline(format!("cannot serialize baseline: {e}")))?;
    std::fs::write(path, format!("{json}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_formatting_but_not_content() {
        let a = fingerprint("Hello   world\n");
        let b = fingerprint("hello world");
        let c = fingerprint("HELLO\tWORLD");
        assert_eq!(a, b, "whitespace and case must normalize away");
        assert_eq!(a, c);
        assert_ne!(a, fingerprint("hello worlds"), "content must still matter");
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn empty_content_still_fingerprints() {
        assert_eq!(fingerprint("").len(), 16);
        assert_eq!(fingerprint(""), fingerprint("   \n\t "));
    }

    #[test]
    fn label_prefers_title_then_truncates() {
        assert_eq!(label_for(Some("A title"), "body"), "A title");
        assert_eq!(label_for(Some("   "), "body text"), "body text");
        assert_eq!(label_for(None, "a\n\nb"), "a b");
        let long = "x".repeat(200);
        let label = label_for(None, &long);
        assert_eq!(label.chars().count(), 81, "80 chars plus the ellipsis");
        assert!(label.ends_with('…'));
    }

    #[test]
    fn machine_id_is_filename_safe() {
        let id = machine_id();
        assert!(!id.is_empty());
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "machine id {id} must be safe to embed in a fixture filename"
        );
    }
}
