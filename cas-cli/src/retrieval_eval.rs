//! Labeled retrieval-evaluation harness (cas-e0ed, EPIC cas-8fac layer 1).
//!
//! `cas-cli/tests/retrieval_parity_test.rs` already replays captured queries
//! and reports whether retrieval *changed*. It carries no relevance labels, so
//! it cannot say whether retrieval is *good*. This module is the missing half:
//! a committed fixture of hand-judged (prompt-context → relevant-entry) pairs,
//! replayed through the two selectors that actually put memories in front of a
//! model, scored with precision@5 / recall@5 against a committed baseline.
//!
//! # What is measured
//!
//! * **`helpful_memories_production`** (cas-b06c) — the REAL SessionStart path:
//!   `cas-cli/src/hooks/context.rs` opens
//!   `HybridContextScorer::open_with_graph` and passes it as `entry_scorer`, so
//!   ranking is `hs * 0.7 + basic * 0.3` plus `contextual_overlap_bonus`. This
//!   is the row that describes what a session actually receives. It is measured
//!   in both [`QueryMode`]s, because a SessionStart's `ContextQuery` is only
//!   non-empty when an in-progress task exists.
//! * **`helpful_memories`** — the Basic FALLBACK control. Built by
//!   [`cas_core::hooks::context::build_context_with_stores`] with no
//!   `entry_scorer`, which is `build_start.rs`'s fallback. Retained from
//!   cas-e0ed so that a change pushing production back onto the fallback shows
//!   up as the two rows converging, rather than as one unexplained move.
//!   Ranking is captured through the production `on_surfaced` callback, in
//!   render order, at the production limit.
//! * **`ambient_packet`** — the evidence cards that
//!   [`crate::ambient_recall::render_packet`] actually injects, in injection
//!   order, after the real candidate retrieval, fusion, scope gate and
//!   injection budget.
//! * **`ambient_candidates`** — a diagnostic third view: the ranked candidate
//!   list *before* `render_packet`'s injection budget. It exists because the
//!   packet caps lexical-only injections at
//!   `ambient_recall::LEXICAL_INJECTION_CAP` (3), so `ambient_packet` recall@5
//!   is structurally capped on an install with no embedding cache. Separating
//!   the two keeps a retrieval regression distinguishable from a budget change.
//!
//! # Tier modes
//!
//! The Helpful-Memories selector only considers entries whose
//! `MemoryTier::is_active()` is true (`in-context` / `working`). In the real
//! corpus 175 of the 189 fixture entries are `archive`, so the shipped
//! selector can physically see 14 of them. That is a genuine finding, not a
//! fixture defect, so the harness reports both:
//!
//! * `live_tiers` — fixture tiers exactly as mined. Measures the shipped
//!   end-to-end selector, tier filter included.
//! * `all_working` — every entry lifted to `working`. Measures the ranking
//!   function alone, with the tier filter neutralised, so a ranking change
//!   (P6b boosts) has a metric that can move.
//!
//! The ambient selectors filter on `archived = 0` only, so their two modes are
//! expected to agree; a divergence there is itself a signal.
//!
//! # Determinism
//!
//! `BasicContextScorer` decays by wall-clock age and boosts by `last_accessed`,
//! so absolute timestamps would make the baseline rot. The fixture therefore
//! stores `age_days` rather than a `created` instant, and materialisation sets
//! `created = now - age_days - <fixture position> seconds`. Relative ages are
//! preserved exactly, every timestamp is unique (the store's
//! `ORDER BY created DESC` has no tiebreaker), and `num_days()` still yields
//! `age_days`. `updated_at` is rewritten to equal `created` after seeding so
//! the ambient retriever's `ORDER BY coalesce(updated_at, created) DESC` sees
//! the real recency order rather than insertion order.
//!
//! # Metrics
//!
//! For a case with labeled relevant set `R` and a selector that returned the
//! ranked list `S` (truncated to 5):
//!
//! * `precision@5 = |R ∩ S| / min(5, |S|)` — the denominator is the number of
//!   slots the selector actually used, not a flat 5. A selector that is
//!   structurally capped below 5 slots (the ambient packet) would otherwise be
//!   penalised for a budget decision rather than for a ranking decision.
//!   `mean_returned` is reported alongside so the cap stays visible.
//! * `recall@5 = |R ∩ S| / |R|`.
//! * Cases where the selector returned nothing contribute `0.0` precision —
//!   silence is a retrieval outcome, not an excused case.
//! * The `lenient_*` variants additionally count `ambiguous` labels as
//!   relevant. Strict is the gated metric; lenient is the honest upper bound
//!   for pairs the judge would not swear to.
//! * `distinct_rankings` counts how many different top-5 lists a selector
//!   produced across all cases. It is the direct measure of query-dependence:
//!   `1` means the selector returns the same memories no matter what the
//!   session is about. It is in the baseline, not just in a test, because a
//!   move between `1` and `n` is exactly the event this harness exists to
//!   catch.
//!
//! # Re-baselining
//!
//! **One-line procedure:** run
//! `CAS_RETRIEVAL_EVAL_REBASELINE=1 cargo test -p cas --test retrieval_eval_test`
//! — it rewrites `cas-cli/tests/data/retrieval-eval/baseline.json` in place;
//! commit that file in the same commit as the change that moved the numbers,
//! with the reason in the commit message.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use cas_store::{SqliteStore, Store};
use cas_types::{Entry, EntryType, MemoryTier, Scope};

use crate::ambient_recall::{
    RecallIdentity, RecallLedger, RecallRequest, RecallRole, RecallRetriever, SqliteRecallRetriever,
    render_packet, retrieve_candidates,
};

/// The production SessionStart limit (`hooks/handlers/handlers_session.rs`).
pub const SESSION_START_LIMIT: usize = 5;

/// The evaluation cutoff. Both metrics are @K with this K.
pub const K: usize = 5;

/// Relative regression tolerance for the committed gate: a selector may lose at
/// most this fraction of its baseline score before the gate fails.
pub const REGRESSION_TOLERANCE: f64 = 0.10;

/// Environment switch that rewrites the committed baseline instead of gating.
pub const REBASELINE_ENV: &str = "CAS_RETRIEVAL_EVAL_REBASELINE";

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// One snapshotted memory. Self-contained: the harness never reads a live store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureEntry {
    pub id: String,
    pub entry_type: String,
    pub memory_tier: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub importance: f32,
    pub stability: f32,
    pub helpful_count: i32,
    pub harmful_count: i32,
    /// Days between the entry's real `created` date and the fixture reference
    /// date. See the module docs for why this replaces an absolute timestamp.
    pub age_days: i64,
    pub title: String,
    pub body: String,
}

/// One judged prompt-context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    pub case_id: String,
    pub task_id: String,
    pub task_title: String,
    #[serde(default)]
    pub task_labels: Vec<String>,
    pub user_prompt: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub files: Vec<String>,
    /// Entry ids a judge asserts are materially relevant to this context.
    pub relevant: Vec<String>,
    /// Topically adjacent but a judgment call. Excluded from the strict metric.
    #[serde(default)]
    pub ambiguous: Vec<String>,
    pub judged_by: String,
    pub judged_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalFixture {
    pub version: u32,
    pub fixture_id: String,
    pub reference_date: String,
    pub judged_by: String,
    pub judged_at: String,
    #[serde(default)]
    pub provenance: serde_json::Value,
    pub entries: Vec<FixtureEntry>,
    pub cases: Vec<EvalCase>,
}

/// Fixture format the harness understands. Bump together with a reader change.
pub const FIXTURE_VERSION: u32 = 1;

#[derive(Debug)]
pub enum EvalError {
    Io(String),
    Parse(String),
    Store(String),
    Version { found: u32, expected: u32 },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "retrieval-eval io error: {m}"),
            Self::Parse(m) => write!(f, "retrieval-eval parse error: {m}"),
            Self::Store(m) => write!(f, "retrieval-eval store error: {m}"),
            Self::Version { found, expected } => write!(
                f,
                "retrieval-eval fixture version {found} is not the expected {expected}"
            ),
        }
    }
}

impl std::error::Error for EvalError {}

impl EvalFixture {
    pub fn load(path: &Path) -> Result<Self, EvalError> {
        let bytes = std::fs::read(path)
            .map_err(|e| EvalError::Io(format!("{}: {e}", path.display())))?;
        let fixture: Self = serde_json::from_slice(&bytes)
            .map_err(|e| EvalError::Parse(format!("{}: {e}", path.display())))?;
        if fixture.version != FIXTURE_VERSION {
            return Err(EvalError::Version {
                found: fixture.version,
                expected: FIXTURE_VERSION,
            });
        }
        Ok(fixture)
    }

    /// Path of the committed fixture, resolved from the workspace root.
    pub fn committed_path() -> PathBuf {
        crate::test_paths::workspace_root()
            .join("cas-cli/tests/data/retrieval-eval/fixture.json")
    }

    /// Path of the committed baseline.
    pub fn committed_baseline_path() -> PathBuf {
        crate::test_paths::workspace_root()
            .join("cas-cli/tests/data/retrieval-eval/baseline.json")
    }
}

// ---------------------------------------------------------------------------
// Corpus materialisation
// ---------------------------------------------------------------------------

/// Which memory tiers the materialised corpus carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierMode {
    /// Tiers exactly as mined from the live store.
    Live,
    /// Every entry lifted to `working`, neutralising the tier filter.
    AllWorking,
}

impl TierMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live_tiers",
            Self::AllWorking => "all_working",
        }
    }
}

/// Which SessionStart shape the production selector is measured under.
///
/// `ContextQuery::has_content()` (cas-core/src/hooks/context/mod.rs:113) is
/// `!task_titles.is_empty() || user_prompt.is_some() || !recent_files.is_empty()`
/// — **cwd is deliberately not counted**, even though `to_query_string()`
/// includes it. At SessionStart `user_prompt` is always `None` (that field
/// belongs to UserPromptSubmit), and `recent_files` comes from
/// `<cas_root>/session_files.json`, which does not exist on the live cas-src
/// store. So in practice the only thing that can make a real SessionStart
/// query-aware is an in-progress task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryMode {
    /// The factory regime: one in-progress task, as a worker session sees.
    SeededTask,
    /// A fresh session in a project: no in-progress task, no session files.
    FreshSession,
}

impl QueryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SeededTask => "seeded_task",
            Self::FreshSession => "fresh_session",
        }
    }
}

/// Which branch of `HybridContextScorer::score_entries` a SessionStart lands on.
///
/// Conflating these is how "we ship a hybrid scorer" becomes "we ship Basic and
/// never notice", so the harness reports the branch rather than inferring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionScorerState {
    /// `HybridContextScorer::open_with_graph` returned Err, so `build_start`
    /// uses `BasicContextScorer` with no overlap bonus at all.
    ///
    /// Effectively unreachable in the field: `SearchIndex::open` *creates* a
    /// missing index directory rather than failing
    /// (hybrid_search/search_index_impl.rs:73-96), and `open_with_graph`
    /// swallows an entity-store error. Kept as a distinct state because the
    /// audit brief predicted it, and "we checked and it cannot happen" is a
    /// different answer from "it happens".
    ScorerUnavailable,
    /// `scorer.rs:123` early-returned pure Basic because `has_content()` was
    /// false. The `contextual_overlap_bonus` loop at `:173` never runs, so this
    /// is the genuinely query-blind state.
    QueryBlindEarlyReturn,
    /// The query had content, but the BM25 index holds no documents, so no
    /// lexical evidence is possible.
    ///
    /// Measured, not assumed: this state is still query-*dependent*. The
    /// hybrid search's temporal channel needs no index and returns results
    /// anyway, so `score_with_hybrid` is non-empty and production takes the
    /// hybrid branch ranking on recency + graph + basic, and then adds
    /// `contextual_overlap_bonus` on top. The audit brief's prediction that a
    /// missing index means "silent Basic, query-blind" is wrong twice over.
    QueryAwareWithoutLexicalIndex,
    /// The query had content and the BM25 index holds documents: the full
    /// production path, `hs * 0.7 + basic * 0.3` plus the bonus.
    QueryAwareWithLexicalIndex,
}

impl ProductionScorerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScorerUnavailable => "scorer_unavailable",
            Self::QueryBlindEarlyReturn => "query_blind_early_return",
            Self::QueryAwareWithoutLexicalIndex => "query_aware_without_lexical_index",
            Self::QueryAwareWithLexicalIndex => "query_aware_with_lexical_index",
        }
    }

    /// Whether ranking on this branch can vary with the prompt context at all.
    pub fn is_query_dependent(self) -> bool {
        matches!(
            self,
            Self::QueryAwareWithoutLexicalIndex | Self::QueryAwareWithLexicalIndex
        )
    }
}

/// Live documents in the BM25 index `HybridSearch` actually reads.
///
/// Read from the Tantivy `meta.json` at `<cas_dir>/index/tantivy` rather than
/// through a search, because a search cannot distinguish "no lexical hits" from
/// "no lexical index": the temporal channel returns results either way.
/// Returns 0 when the index does not exist.
pub fn indexed_document_count(cas_dir: &Path) -> usize {
    let meta_path = cas_dir.join("index").join("tantivy").join("meta.json");
    let Ok(bytes) = std::fs::read(&meta_path) else {
        return 0;
    };
    let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return 0;
    };
    meta.get("segments")
        .and_then(|s| s.as_array())
        .map(|segments| {
            segments
                .iter()
                .map(|segment| {
                    let max_doc = segment
                        .get("max_doc")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let deleted = segment
                        .get("deletes")
                        .and_then(|d| d.get("num_deleted_docs"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    max_doc.saturating_sub(deleted) as usize
                })
                .sum()
        })
        .unwrap_or(0)
}

/// A materialised, disposable CAS store holding exactly the fixture entries.
pub struct EvalCorpus {
    cas_dir: PathBuf,
    active_entries: usize,
    has_search_index: bool,
}

impl EvalCorpus {
    pub fn cas_dir(&self) -> &Path {
        &self.cas_dir
    }

    /// Entries the Helpful-Memories tier filter can physically see.
    pub fn active_entries(&self) -> usize {
        self.active_entries
    }

    /// Whether a real Tantivy index was built over this corpus.
    pub fn has_search_index(&self) -> bool {
        self.has_search_index
    }

    /// Seed the corpus and additionally build a real Tantivy index over it.
    ///
    /// The index is written through `HybridSearch::open` + `index_entry` — the
    /// same production code the inline write sites use — so it lands in
    /// `<cas_root>/index/tantivy`, exactly where
    /// `HybridContextScorer::open_with_graph` reads from
    /// (hybrid_search/hybrid.rs:370). Deliberately NOT `BackgroundIndexer`,
    /// which writes to `<cas_root>/index` and is read by nothing.
    pub fn materialize_with_index(
        fixture: &EvalFixture,
        cas_dir: &Path,
        mode: TierMode,
    ) -> Result<Self, EvalError> {
        let mut corpus = Self::materialize(fixture, cas_dir, mode)?;

        let store = SqliteStore::open(cas_dir)
            .map_err(|e| EvalError::Store(format!("open for index: {e}")))?;
        let entries = store
            .list()
            .map_err(|e| EvalError::Store(format!("list for index: {e}")))?;
        drop(store);

        // `reindex`, not a per-entry `index_entry` loop: each `index_entry`
        // acquires and releases its own Tantivy `IndexWriter`, and the lock
        // file release lags the drop, so a tight loop intermittently fails with
        // `Failed to acquire Lockfile: LockBusy`. One writer, one commit.
        let index = crate::hybrid_search::HybridSearch::open(cas_dir)
            .map_err(|e| EvalError::Store(format!("open search index: {e}")))?;
        index
            .reindex(&entries)
            .map_err(|e| EvalError::Store(format!("index {} entries: {e}", entries.len())))?;
        drop(index);

        corpus.has_search_index = true;
        Ok(corpus)
    }

    /// Seed `cas_dir` (which must be empty) with the fixture corpus.
    pub fn materialize(
        fixture: &EvalFixture,
        cas_dir: &Path,
        mode: TierMode,
    ) -> Result<Self, EvalError> {
        // Ordered oldest-last so `created` descends with the fixture position
        // and each row gets a unique timestamp.
        let mut ordered: Vec<&FixtureEntry> = fixture.entries.iter().collect();
        ordered.sort_by(|a, b| a.age_days.cmp(&b.age_days).then_with(|| a.id.cmp(&b.id)));

        let now = Utc::now();
        let store =
            SqliteStore::open(cas_dir).map_err(|e| EvalError::Store(format!("open: {e}")))?;
        store
            .init()
            .map_err(|e| EvalError::Store(format!("init: {e}")))?;

        let mut active = 0usize;
        for (position, fe) in ordered.iter().enumerate() {
            let entry = build_entry(fe, mode, now, position)?;
            if entry.memory_tier.is_active() {
                active += 1;
            }
            store
                .add(&entry)
                .map_err(|e| EvalError::Store(format!("add {}: {e}", fe.id)))?;
        }
        drop(store);

        // `store_add` stamps `updated_at = now()`, which would make the ambient
        // retriever's `ORDER BY coalesce(updated_at, created) DESC` reflect
        // insertion order instead of the corpus's real recency order.
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db"))
            .map_err(|e| EvalError::Store(format!("reopen: {e}")))?;
        conn.execute("UPDATE entries SET updated_at = created", [])
            .map_err(|e| EvalError::Store(format!("align updated_at: {e}")))?;
        drop(conn);

        Ok(Self {
            cas_dir: cas_dir.to_path_buf(),
            active_entries: active,
            has_search_index: false,
        })
    }
}

fn build_entry(
    fe: &FixtureEntry,
    mode: TierMode,
    now: chrono::DateTime<Utc>,
    position: usize,
) -> Result<Entry, EvalError> {
    let entry_type: EntryType = fe
        .entry_type
        .parse()
        .map_err(|_| EvalError::Parse(format!("{}: bad entry_type {}", fe.id, fe.entry_type)))?;
    let tier: MemoryTier = match mode {
        TierMode::AllWorking => MemoryTier::Working,
        TierMode::Live => fe
            .memory_tier
            .parse()
            .map_err(|_| EvalError::Parse(format!("{}: bad tier {}", fe.id, fe.memory_tier)))?,
    };

    let mut entry = Entry::with_scope(fe.id.clone(), fe.body.clone(), Scope::Project);
    entry.entry_type = entry_type;
    entry.memory_tier = tier;
    entry.tags = fe.tags.clone();
    entry.title = Some(fe.title.clone());
    entry.importance = fe.importance;
    entry.stability = fe.stability;
    entry.helpful_count = fe.helpful_count;
    entry.harmful_count = fe.harmful_count;
    entry.created = now - Duration::days(fe.age_days) - Duration::seconds(position as i64);
    // `last_accessed` drives a wall-clock access boost in BasicContextScorer.
    // The fixture deliberately carries none: an access-time snapshot cannot be
    // replayed stably, and leaving it None keeps the scorer's remaining terms
    // (type, feedback, age, importance, stability) as the whole story.
    entry.last_accessed = None;
    entry.access_count = 0;
    Ok(entry)
}

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

/// Identifiers of the selectors this harness scores.
pub const SELECTOR_HELPFUL_MEMORIES: &str = "helpful_memories";
pub const SELECTOR_AMBIENT_PACKET: &str = "ambient_packet";
pub const SELECTOR_AMBIENT_CANDIDATES: &str = "ambient_candidates";
/// The real SessionStart path: `context.rs` + `HybridContextScorer` (cas-b06c).
pub const SELECTOR_HELPFUL_MEMORIES_PRODUCTION: &str = "helpful_memories_production";

const EVAL_PROJECT_ID: &str = "cas-retrieval-eval-fixture";

fn hook_input(case: &EvalCase) -> cas_core::hooks::types::HookInput {
    cas_core::hooks::types::HookInput {
        session_id: format!("retrieval-eval-{}", case.case_id),
        cwd: case.cwd.clone(),
        hook_event_name: "SessionStart".to_string(),
        user_prompt: Some(case.user_prompt.clone()),
        ..Default::default()
    }
}

/// Rank the entries the SessionStart "Helpful Memories" section would surface.
///
/// This drives the real `build_context_with_stores` and reads the ranking off
/// the production `on_surfaced` callback, so it cannot drift from what a live
/// session receives.
pub fn helpful_memories_ranking(
    corpus: &EvalCorpus,
    case: &EvalCase,
) -> Result<Vec<String>, EvalError> {
    use cas_core::hooks::context::{ContextStores, SurfacedItemCallback};
    use std::sync::{Arc, Mutex};

    let store =
        SqliteStore::open(corpus.cas_dir()).map_err(|e| EvalError::Store(format!("open: {e}")))?;

    let surfaced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&surfaced);
    let callback: SurfacedItemCallback = Box::new(move |id: &str, item_type: &str, _preview| {
        // `ContextItemType::Memory` is the Helpful-Memories section. The
        // "related" item_type is the separate "Related to Current Work"
        // section, which only exists for the hybrid scorer and is scored
        // separately if it is ever wired into this harness.
        if item_type == "Memory"
            && let Ok(mut ids) = sink.lock()
        {
            ids.push(id.to_string());
        }
    });

    let mut stores = ContextStores::empty();
    stores.project_store = Some(&store as &dyn Store);

    let config = crate::config::Config::default();
    let (_context, _stats) = cas_core::hooks::context::build_context_with_stores(
        &hook_input(case),
        &stores,
        &config,
        SESSION_START_LIMIT,
        Some(&callback),
        "mcp__cas__",
    )
    .map_err(|e| EvalError::Store(format!("build_context: {e}")))?;

    let ranked = surfaced
        .lock()
        .map(|ids| ids.clone())
        .unwrap_or_default();
    Ok(ranked)
}

// ---------------------------------------------------------------------------
// The production Helpful-Memories path (cas-b06c)
// ---------------------------------------------------------------------------

/// Clears the factory environment for the duration of a production measurement.
///
/// `build_context_with_stores` renders an Agent Coordination section whenever
/// `CAS_AGENT_ROLE` is set, which consumes token budget and can change which
/// memories still fit inside it. The committed baseline must describe a plain
/// operator session, not whichever pane happened to run the harness.
///
/// Safe under the repo's standard runner: nextest gives every test its own
/// process, so this cannot race a sibling test.
struct NeutralHookEnv {
    restore: Option<String>,
}

impl NeutralHookEnv {
    fn acquire() -> Self {
        let restore = std::env::var("CAS_AGENT_ROLE").ok();
        if restore.is_some() {
            unsafe { std::env::remove_var("CAS_AGENT_ROLE") };
        }
        Self { restore }
    }
}

impl Drop for NeutralHookEnv {
    fn drop(&mut self) {
        if let Some(value) = self.restore.take() {
            unsafe { std::env::set_var("CAS_AGENT_ROLE", value) };
        }
    }
}

/// Installs the case's real task as the session's single in-progress task, and
/// removes it again on drop.
///
/// This is how a real worker session gets a non-empty `ContextQuery`:
/// `build_start.rs` reads `task_titles` from `list(Some(TaskStatus::InProgress))`.
struct SeededTask<'a> {
    cas_dir: &'a Path,
    task_id: Option<String>,
}

impl<'a> SeededTask<'a> {
    fn install(
        cas_dir: &'a Path,
        case: &EvalCase,
        mode: QueryMode,
    ) -> Result<Self, EvalError> {
        if mode == QueryMode::FreshSession {
            return Ok(Self {
                cas_dir,
                task_id: None,
            });
        }
        let store = crate::store::open_task_store_local(cas_dir)
            .map_err(|e| EvalError::Store(format!("open task store: {e}")))?;
        let mut task = cas_types::Task::new(case.task_id.clone(), case.task_title.clone());
        task.status = cas_types::TaskStatus::InProgress;
        task.description = case.user_prompt.clone();
        task.labels = case.task_labels.clone();
        store
            .add(&task)
            .map_err(|e| EvalError::Store(format!("seed task {}: {e}", case.task_id)))?;
        Ok(Self {
            cas_dir,
            task_id: Some(case.task_id.clone()),
        })
    }
}

impl Drop for SeededTask<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.task_id.take()
            && let Ok(store) = crate::store::open_task_store_local(self.cas_dir)
        {
            let _ = store.delete(&id);
        }
    }
}

/// The hook input a real SessionStart carries.
///
/// Note `user_prompt: None` — `handle_session_start` never populates it; that
/// field belongs to UserPromptSubmit. The cas-e0ed Basic control passes a
/// prompt, which is harmless there only because `BasicContextScorer` ignores
/// the query entirely.
fn production_hook_input(case: &EvalCase) -> cas_core::hooks::types::HookInput {
    cas_core::hooks::types::HookInput {
        session_id: format!("retrieval-eval-{}", case.case_id),
        cwd: case.cwd.clone(),
        hook_event_name: "SessionStart".to_string(),
        user_prompt: None,
        ..Default::default()
    }
}

/// The limit production passes, read from the same config path it reads.
fn production_context_limit(cas_dir: &Path) -> usize {
    crate::config::Config::load(cas_dir)
        .unwrap_or_default()
        .context_limit()
}

/// Pull the Helpful-Memories ids out of a rendered SessionStart block.
///
/// The production path returns a rendered string, and that string is literally
/// what the model receives — so parsing it needs no new seam in production
/// code. `the_rendered_section_parser_agrees_with_the_production_callback`
/// cross-validates this against the `on_surfaced` callback so parser drift
/// fails a test instead of silently reshaping the baseline.
pub fn parse_helpful_memories_section(rendered: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut in_section = false;
    for line in rendered.lines() {
        if line.starts_with("## Helpful Memories") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // One blank line separates the header from the list; a second ends
            // the section.
            if ids.is_empty() {
                continue;
            }
            break;
        }
        let Some(rest) = trimmed.strip_prefix("- ") else {
            break;
        };
        match rest.split_whitespace().next() {
            Some(id) => ids.push(id.to_string()),
            None => break,
        }
    }
    ids
}

/// Rank the Helpful Memories the PRODUCTION SessionStart path would inject.
///
/// Drives `crate::hooks::build_context`, which is what
/// `handle_session_start` calls: it opens the stores, opens
/// `HybridContextScorer::open_with_graph`, and passes it as `entry_scorer`.
pub fn helpful_memories_production_ranking(
    corpus: &EvalCorpus,
    case: &EvalCase,
    query_mode: QueryMode,
) -> Result<Vec<String>, EvalError> {
    let _env = NeutralHookEnv::acquire();
    let _task = SeededTask::install(corpus.cas_dir(), case, query_mode)?;

    let rendered = crate::hooks::build_context(
        &production_hook_input(case),
        production_context_limit(corpus.cas_dir()),
        corpus.cas_dir(),
    )
    .map_err(|e| EvalError::Store(format!("build_context: {e}")))?;

    Ok(parse_helpful_memories_section(&rendered))
}

/// The same production wiring as [`helpful_memories_production_ranking`], with
/// the store and scorer opens hoisted out of the per-case loop.
///
/// `crate::hooks::build_context` re-opens six SQLite stores, the Tantivy index
/// and the entity store on every call — measured at ~260 ms per case, which is
/// ~58 s for the full matrix and would put the suite over its runtime budget.
/// This runner opens each of those once and then drives the identical
/// `build_context_with_stores` call with the identical `ContextQuery`.
///
/// The fidelity risk is drift from `context.rs`. That is pinned, not hoped for:
/// `the_fast_production_runner_matches_the_real_build_context_path` asserts the
/// two produce the same ranking case by case. If someone changes the wiring in
/// `context.rs` and not here, that test fails.
pub struct ProductionRunner<'a> {
    corpus: &'a EvalCorpus,
    store: SqliteStore,
    scorer: crate::hooks::scorer::HybridContextScorer,
    config: crate::config::Config,
    limit: usize,
}

impl<'a> ProductionRunner<'a> {
    pub fn open(corpus: &'a EvalCorpus) -> Result<Self, EvalError> {
        let cas_dir = corpus.cas_dir();
        Ok(Self {
            store: SqliteStore::open(cas_dir)
                .map_err(|e| EvalError::Store(format!("open store: {e}")))?,
            scorer: crate::hooks::scorer::HybridContextScorer::open_with_graph(cas_dir)
                .map_err(|e| EvalError::Store(format!("open scorer: {e}")))?,
            config: crate::config::Config::load(cas_dir).unwrap_or_default(),
            limit: production_context_limit(cas_dir),
            corpus,
        })
    }

    /// Rank one case exactly as `context.rs` would, reusing the open handles.
    pub fn rank(&self, case: &EvalCase, query_mode: QueryMode) -> Result<Vec<String>, EvalError> {
        use cas_core::hooks::context::{ContextScorer, ContextStores, SurfacedItemCallback};
        use std::sync::{Arc, Mutex};

        let _env = NeutralHookEnv::acquire();
        let _task = SeededTask::install(self.corpus.cas_dir(), case, query_mode)?;

        let task_store = crate::store::open_task_store_local(self.corpus.cas_dir())
            .map_err(|e| EvalError::Store(format!("open task store: {e}")))?;

        let surfaced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&surfaced);
        let callback: SurfacedItemCallback = Box::new(move |id: &str, item_type: &str, _preview| {
            if item_type == "Memory"
                && let Ok(mut ids) = sink.lock()
            {
                ids.push(id.to_string());
            }
        });

        let mut stores = ContextStores::empty();
        stores.project_store = Some(&self.store as &dyn Store);
        stores.task_store = Some(task_store.as_ref());
        stores.entry_scorer = Some(&self.scorer as &dyn ContextScorer);
        stores.recent_files = crate::hooks::handlers::get_session_files(self.corpus.cas_dir());

        cas_core::hooks::context::build_context_with_stores(
            &production_hook_input(case),
            &stores,
            &self.config,
            self.limit,
            Some(&callback),
            "mcp__cas__",
        )
        .map_err(|e| EvalError::Store(format!("build_context_with_stores: {e}")))?;

        Ok(surfaced.lock().map(|ids| ids.clone()).unwrap_or_default())
    }
}

/// The Basic-path ranking read back out of the rendered block rather than the
/// callback. Exists only to cross-validate [`parse_helpful_memories_section`].
pub fn helpful_memories_rendered_ranking(
    corpus: &EvalCorpus,
    case: &EvalCase,
) -> Result<Vec<String>, EvalError> {
    use cas_core::hooks::context::ContextStores;

    let store = SqliteStore::open(corpus.cas_dir())
        .map_err(|e| EvalError::Store(format!("open: {e}")))?;
    let mut stores = ContextStores::empty();
    stores.project_store = Some(&store as &dyn Store);

    let config = crate::config::Config::default();
    let (rendered, _stats) = cas_core::hooks::context::build_context_with_stores(
        &hook_input(case),
        &stores,
        &config,
        SESSION_START_LIMIT,
        None,
        "mcp__cas__",
    )
    .map_err(|e| EvalError::Store(format!("build_context: {e}")))?;

    Ok(parse_helpful_memories_section(&rendered))
}

/// Report which branch of `HybridContextScorer::score_entries` this case takes.
///
/// Every discriminator here is an observation of a real artifact — the scorer's
/// own constructor, the `ContextQuery` production would build, and the document
/// count in the index `HybridSearch` reads — rather than a re-implementation of
/// `score_with_hybrid`. An earlier version of this probe ran a mirrored search
/// and treated a non-empty result as proof of lexical evidence; that was wrong,
/// because the temporal channel returns results with an empty index.
pub fn probe_production_scorer_state(
    corpus: &EvalCorpus,
    case: &EvalCase,
    query_mode: QueryMode,
) -> ProductionScorerState {
    use cas_core::hooks::context::ContextQuery;
    use cas_types::TaskStatus;

    let _env = NeutralHookEnv::acquire();
    let Ok(_task) = SeededTask::install(corpus.cas_dir(), case, query_mode) else {
        return ProductionScorerState::ScorerUnavailable;
    };

    if crate::hooks::scorer::HybridContextScorer::open_with_graph(corpus.cas_dir()).is_err() {
        return ProductionScorerState::ScorerUnavailable;
    }

    let task_titles: Vec<String> = crate::store::open_task_store_local(corpus.cas_dir())
        .ok()
        .and_then(|ts| ts.list(Some(TaskStatus::InProgress)).ok())
        .map(|tasks| tasks.iter().map(|t| t.title.clone()).collect())
        .unwrap_or_default();
    let query = ContextQuery {
        task_titles,
        cwd: case.cwd.clone(),
        user_prompt: None,
        recent_files: crate::hooks::handlers::get_session_files(corpus.cas_dir()),
    };
    if !query.has_content() || query.to_query_string().trim().is_empty() {
        return ProductionScorerState::QueryBlindEarlyReturn;
    }

    if indexed_document_count(corpus.cas_dir()) == 0 {
        ProductionScorerState::QueryAwareWithoutLexicalIndex
    } else {
        ProductionScorerState::QueryAwareWithLexicalIndex
    }
}

fn recall_identity(case: &EvalCase) -> RecallIdentity {
    RecallIdentity {
        session_id: format!("retrieval-eval-{}", case.case_id),
        agent_name: "retrieval-eval-worker".to_string(),
        factory_session: "retrieval-eval-session".to_string(),
        role: RecallRole::Worker,
        project_id: EVAL_PROJECT_ID.to_string(),
        team_id: None,
        internal_llm: false,
    }
}

fn recall_request(case: &EvalCase) -> RecallRequest {
    RecallRequest {
        prompt: case.user_prompt.clone(),
        task_id: Some(case.task_id.clone()),
        task_title: Some(case.task_title.clone()),
        task_labels: case.task_labels.clone(),
        files: case.files.clone(),
        ..Default::default()
    }
}

/// Rank the evidence the ambient packet would actually inject, in packet order.
pub fn ambient_packet_ranking(corpus: &EvalCorpus, case: &EvalCase) -> Vec<String> {
    let identity = recall_identity(case);
    let request = recall_request(case);
    let Some(retriever) = SqliteRecallRetriever::existing(corpus.cas_dir()) else {
        return Vec::new();
    };
    let retrievers: Vec<&dyn RecallRetriever> = vec![&retriever];
    let Some(candidates) = retrieve_candidates(&identity, &request, &retrievers) else {
        return Vec::new();
    };
    let Some(query) = crate::ambient_recall::RecallQuery::build(&identity, &request) else {
        return Vec::new();
    };
    let mut ledger = RecallLedger::default();
    match render_packet(&identity, &query, &candidates, &mut ledger) {
        Some((_packet, injected)) => injected
            .into_iter()
            .map(|candidate| candidate.evidence_id)
            .collect(),
        None => Vec::new(),
    }
}

/// Rank the ambient candidates *before* the packet's injection budget.
pub fn ambient_candidate_ranking(corpus: &EvalCorpus, case: &EvalCase) -> Vec<String> {
    let identity = recall_identity(case);
    let request = recall_request(case);
    let Some(retriever) = SqliteRecallRetriever::existing(corpus.cas_dir()) else {
        return Vec::new();
    };
    let retrievers: Vec<&dyn RecallRetriever> = vec![&retriever];
    match retrieve_candidates(&identity, &request, &retrievers) {
        Some(candidates) => candidates
            .candidates
            .into_iter()
            .map(|candidate| candidate.evidence_id)
            .collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Per-case scoring detail, kept out of the baseline but printed on failure.
#[derive(Debug, Clone, Serialize)]
pub struct CaseScore {
    pub case_id: String,
    pub returned: usize,
    pub hits: usize,
    pub relevant: usize,
    pub precision: f64,
    pub recall: f64,
    pub top_k: Vec<String>,
}

/// Aggregate metrics for one (selector, tier mode) pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectorMetrics {
    pub selector: String,
    pub tier_mode: String,
    /// SessionStart shape this row was measured under, or `n/a` for selectors
    /// that do not read the SessionStart `ContextQuery` (the Basic control and
    /// both ambient selectors, which build their own query).
    #[serde(default = "not_applicable")]
    pub query_mode: String,
    pub cases: usize,
    pub precision_at_5: f64,
    pub recall_at_5: f64,
    pub lenient_precision_at_5: f64,
    pub lenient_recall_at_5: f64,
    /// Mean number of results the selector returned within the @5 window.
    /// A value below 5 means the selector, not the fixture, chose to stay quiet.
    pub mean_returned: f64,
    /// Cases where at least one labeled-relevant entry made the top 5.
    pub cases_with_a_hit: usize,
    /// Cases where the selector returned nothing at all.
    pub silent_cases: usize,
    /// Number of DISTINCT top-5 rankings across all cases.
    ///
    /// This is the direct measure of query-dependence, and the reason it is in
    /// the baseline rather than only in a test: `1` means the selector returns
    /// the same memories no matter what the session is about, and a change from
    /// `1` to `n` (or back) is exactly the event this harness exists to catch.
    #[serde(default)]
    pub distinct_rankings: usize,
}

fn not_applicable() -> String {
    "n/a".to_string()
}

impl SelectorMetrics {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.selector, self.tier_mode, self.query_mode)
    }
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// Score one selector over every case, with no SessionStart query mode.
pub fn score(
    selector: &str,
    mode: TierMode,
    cases: &[EvalCase],
    rank: impl FnMut(&EvalCase) -> Vec<String>,
) -> (SelectorMetrics, Vec<CaseScore>) {
    score_in_query_mode(selector, mode, None, cases, rank)
}

/// Score one selector over every case.
pub fn score_in_query_mode(
    selector: &str,
    mode: TierMode,
    query_mode: Option<QueryMode>,
    cases: &[EvalCase],
    mut rank: impl FnMut(&EvalCase) -> Vec<String>,
) -> (SelectorMetrics, Vec<CaseScore>) {
    let mut precision_sum = 0.0;
    let mut recall_sum = 0.0;
    let mut lenient_precision_sum = 0.0;
    let mut lenient_recall_sum = 0.0;
    let mut returned_sum = 0.0;
    let mut with_a_hit = 0usize;
    let mut silent = 0usize;
    let mut distinct: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut details = Vec::with_capacity(cases.len());

    for case in cases {
        let ranked = rank(case);
        let top: Vec<String> = ranked.into_iter().take(K).collect();
        let strict: BTreeSet<&str> = case.relevant.iter().map(String::as_str).collect();
        let lenient: BTreeSet<&str> = case
            .relevant
            .iter()
            .chain(case.ambiguous.iter())
            .map(String::as_str)
            .collect();

        let hits = top.iter().filter(|id| strict.contains(id.as_str())).count();
        let lenient_hits = top
            .iter()
            .filter(|id| lenient.contains(id.as_str()))
            .count();

        // Denominator is the slots the selector used, so a structurally capped
        // selector is judged on what it put there, not on what it withheld.
        let denominator = top.len().max(1) as f64;
        let precision = if top.is_empty() {
            0.0
        } else {
            hits as f64 / denominator
        };
        let lenient_precision = if top.is_empty() {
            0.0
        } else {
            lenient_hits as f64 / denominator
        };
        let recall = if strict.is_empty() {
            0.0
        } else {
            hits as f64 / strict.len() as f64
        };
        let lenient_recall = if lenient.is_empty() {
            0.0
        } else {
            lenient_hits as f64 / lenient.len() as f64
        };

        if top.is_empty() {
            silent += 1;
        }
        if hits > 0 {
            with_a_hit += 1;
        }
        precision_sum += precision;
        recall_sum += recall;
        lenient_precision_sum += lenient_precision;
        lenient_recall_sum += lenient_recall;
        returned_sum += top.len() as f64;

        distinct.insert(top.clone());
        details.push(CaseScore {
            case_id: case.case_id.clone(),
            returned: top.len(),
            hits,
            relevant: strict.len(),
            precision: round6(precision),
            recall: round6(recall),
            top_k: top,
        });
    }

    let n = cases.len().max(1) as f64;
    let metrics = SelectorMetrics {
        selector: selector.to_string(),
        tier_mode: mode.as_str().to_string(),
        query_mode: query_mode.map_or_else(not_applicable, |m| m.as_str().to_string()),
        cases: cases.len(),
        precision_at_5: round6(precision_sum / n),
        recall_at_5: round6(recall_sum / n),
        lenient_precision_at_5: round6(lenient_precision_sum / n),
        lenient_recall_at_5: round6(lenient_recall_sum / n),
        mean_returned: round6(returned_sum / n),
        cases_with_a_hit: with_a_hit,
        silent_cases: silent,
        distinct_rankings: distinct.len(),
    };
    (metrics, details)
}

/// Per-`(selector, tier_mode)` case breakdown, keyed by [`SelectorMetrics::key`].
pub type CaseBreakdown = Vec<(String, Vec<CaseScore>)>;

/// Run every selector in every mode against a freshly materialised fixture.
///
/// Owns its temp directories, so the harness can never touch a live store or
/// the user's `~/.cas`. Each tier mode gets a corpus WITH a real Tantivy index,
/// because that is what production reads; the Basic control and the ambient
/// selectors are unaffected by its presence.
pub fn run_all(
    fixture: &EvalFixture,
) -> Result<(Vec<SelectorMetrics>, CaseBreakdown), EvalError> {
    let mut metrics = Vec::new();
    let mut details = Vec::new();

    for mode in [TierMode::Live, TierMode::AllWorking] {
        let dir = tempfile::tempdir()
            .map_err(|e| EvalError::Io(format!("tempdir for {}: {e}", mode.as_str())))?;
        let corpus = EvalCorpus::materialize_with_index(fixture, dir.path(), mode)?;

        // The production path, in both SessionStart shapes. This is the row
        // that describes what a real session receives.
        let runner = ProductionRunner::open(&corpus)?;
        for query_mode in [QueryMode::SeededTask, QueryMode::FreshSession] {
            let (m, d) = score_in_query_mode(
                SELECTOR_HELPFUL_MEMORIES_PRODUCTION,
                mode,
                Some(query_mode),
                &fixture.cases,
                |case| runner.rank(case, query_mode).unwrap_or_default(),
            );
            details.push((m.key(), d));
            metrics.push(m);
        }

        // The Basic fallback control (cas-e0ed). Kept so a change that pushes
        // production back onto the fallback shows up as the two rows
        // converging rather than as a single unexplained move.
        let (m, d) = score(SELECTOR_HELPFUL_MEMORIES, mode, &fixture.cases, |case| {
            helpful_memories_ranking(&corpus, case).unwrap_or_default()
        });
        details.push((m.key(), d));
        metrics.push(m);

        let (m, d) = score(SELECTOR_AMBIENT_PACKET, mode, &fixture.cases, |case| {
            ambient_packet_ranking(&corpus, case)
        });
        details.push((m.key(), d));
        metrics.push(m);

        let (m, d) = score(SELECTOR_AMBIENT_CANDIDATES, mode, &fixture.cases, |case| {
            ambient_candidate_ranking(&corpus, case)
        });
        details.push((m.key(), d));
        metrics.push(m);
    }

    Ok((metrics, details))
}

// ---------------------------------------------------------------------------
// Baseline + gate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,
    pub fixture_id: String,
    pub captured_at: String,
    pub note: String,
    pub selectors: Vec<SelectorMetrics>,
}

pub const BASELINE_VERSION: u32 = 1;

impl Baseline {
    pub fn load(path: &Path) -> Result<Self, EvalError> {
        let bytes = std::fs::read(path)
            .map_err(|e| EvalError::Io(format!("{}: {e}", path.display())))?;
        let baseline: Self = serde_json::from_slice(&bytes)
            .map_err(|e| EvalError::Parse(format!("{}: {e}", path.display())))?;
        if baseline.version != BASELINE_VERSION {
            return Err(EvalError::Version {
                found: baseline.version,
                expected: BASELINE_VERSION,
            });
        }
        Ok(baseline)
    }

    pub fn save(&self, path: &Path) -> Result<(), EvalError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| EvalError::Io(format!("{}: {e}", parent.display())))?;
        }
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|e| EvalError::Parse(format!("serialize baseline: {e}")))?;
        json.push('\n');
        std::fs::write(path, json).map_err(|e| EvalError::Io(format!("{}: {e}", path.display())))
    }

    pub fn get(&self, key: &str) -> Option<&SelectorMetrics> {
        self.selectors.iter().find(|m| m.key() == key)
    }
}

/// One metric that moved further down than the tolerance permits.
#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub key: String,
    pub metric: &'static str,
    pub baseline: f64,
    pub current: f64,
    pub relative_drop: f64,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {:.4} -> {:.4} ({:.1}% relative drop, tolerance {:.0}%)",
            self.key,
            self.metric,
            self.baseline,
            self.current,
            self.relative_drop * 100.0,
            REGRESSION_TOLERANCE * 100.0
        )
    }
}

/// Missing coverage: a baseline entry with no matching current measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct MissingMeasurement {
    pub key: String,
}

/// Compare a fresh run against the committed baseline.
///
/// A baselined `(selector, tier_mode)` pair that is absent from the current run
/// is reported rather than skipped: silently dropping a selector is exactly how
/// a gate stops gating.
pub fn compare(
    baseline: &Baseline,
    current: &[SelectorMetrics],
    tolerance: f64,
) -> (Vec<Regression>, Vec<MissingMeasurement>) {
    let mut regressions = Vec::new();
    let mut missing = Vec::new();

    for expected in &baseline.selectors {
        let key = expected.key();
        let Some(actual) = current.iter().find(|m| m.key() == key) else {
            missing.push(MissingMeasurement { key });
            continue;
        };
        for (metric, base, now) in [
            ("precision@5", expected.precision_at_5, actual.precision_at_5),
            ("recall@5", expected.recall_at_5, actual.recall_at_5),
        ] {
            // A baseline of zero has no relative drop to measure; any move can
            // only be upward. Guarding here keeps the gate from dividing by
            // zero and from blessing a metric that was never informative.
            if base <= 0.0 {
                continue;
            }
            let drop = (base - now) / base;
            if drop > tolerance {
                regressions.push(Regression {
                    key: key.clone(),
                    metric,
                    baseline: base,
                    current: now,
                    relative_drop: drop,
                });
            }
        }
    }

    (regressions, missing)
}

/// Human-readable table, printed by the test on every run.
pub fn render_table(metrics: &[SelectorMetrics]) -> String {
    let mut out = String::new();
    out.push_str(
        "selector                    tier          query          P@5     R@5     lenP@5  ret   hit  silent  distinct\n",
    );
    out.push_str(
        "--------------------------- ------------- -------------  ------  ------  ------  ----  ---  ------  --------\n",
    );
    for m in metrics {
        out.push_str(&format!(
            "{:<27} {:<13} {:<13}  {:.4}  {:.4}  {:.4}  {:.2}  {:>3}  {:>6}  {:>8}\n",
            m.selector,
            m.tier_mode,
            m.query_mode,
            m.precision_at_5,
            m.recall_at_5,
            m.lenient_precision_at_5,
            m.mean_returned,
            m.cases_with_a_hit,
            m.silent_cases,
            m.distinct_rankings,
        ));
    }
    out.push_str(
        "\ndistinct = number of different top-5 rankings across all cases. 1 = the\n\
         selector returns the same memories regardless of what the session is about.\n",
    );
    out
}
