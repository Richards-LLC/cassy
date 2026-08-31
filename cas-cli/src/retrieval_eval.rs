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
//! * **`helpful_memories`** — the SessionStart "Helpful Memories" section built
//!   by [`cas_core::hooks::context::build_context_with_stores`]. The ranking is
//!   captured through the production `on_surfaced` callback, in render order,
//!   with the production `limit = 5` (see `hooks/handlers/handlers_session.rs`).
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

/// A materialised, disposable CAS store holding exactly the fixture entries.
pub struct EvalCorpus {
    cas_dir: PathBuf,
    active_entries: usize,
}

impl EvalCorpus {
    pub fn cas_dir(&self) -> &Path {
        &self.cas_dir
    }

    /// Entries the Helpful-Memories tier filter can physically see.
    pub fn active_entries(&self) -> usize {
        self.active_entries
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
}

impl SelectorMetrics {
    pub fn key(&self) -> String {
        format!("{}/{}", self.selector, self.tier_mode)
    }
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// Score one selector over every case.
pub fn score(
    selector: &str,
    mode: TierMode,
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
        cases: cases.len(),
        precision_at_5: round6(precision_sum / n),
        recall_at_5: round6(recall_sum / n),
        lenient_precision_at_5: round6(lenient_precision_sum / n),
        lenient_recall_at_5: round6(lenient_recall_sum / n),
        mean_returned: round6(returned_sum / n),
        cases_with_a_hit: with_a_hit,
        silent_cases: silent,
    };
    (metrics, details)
}

/// Per-`(selector, tier_mode)` case breakdown, keyed by [`SelectorMetrics::key`].
pub type CaseBreakdown = Vec<(String, Vec<CaseScore>)>;

/// Run every selector in every tier mode against a materialised fixture.
///
/// The caller owns the two temp directories so the harness never touches a
/// live store or the user's `~/.cas`.
pub fn run_all(
    fixture: &EvalFixture,
    live_dir: &Path,
    all_working_dir: &Path,
) -> Result<(Vec<SelectorMetrics>, CaseBreakdown), EvalError> {
    let mut metrics = Vec::new();
    let mut details = Vec::new();

    for (mode, dir) in [
        (TierMode::Live, live_dir),
        (TierMode::AllWorking, all_working_dir),
    ] {
        let corpus = EvalCorpus::materialize(fixture, dir, mode)?;

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
        "selector              tier          cases  P@5     R@5     lenP@5  lenR@5  ret   hit  silent\n",
    );
    out.push_str(
        "--------------------- ------------- -----  ------  ------  ------  ------  ----  ---  ------\n",
    );
    for m in metrics {
        out.push_str(&format!(
            "{:<21} {:<13} {:>5}  {:.4}  {:.4}  {:.4}  {:.4}  {:.2}  {:>3}  {:>6}\n",
            m.selector,
            m.tier_mode,
            m.cases,
            m.precision_at_5,
            m.recall_at_5,
            m.lenient_precision_at_5,
            m.lenient_recall_at_5,
            m.mean_returned,
            m.cases_with_a_hit,
            m.silent_cases,
        ));
    }
    out
}
