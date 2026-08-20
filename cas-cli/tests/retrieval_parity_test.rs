//! End-to-end tests for the retrieval-parity harness (cas-90fd).
//!
//! The load-bearing test here is [`replay_detects_a_deleted_memory`]: a
//! regression detector that has never been shown to detect a regression is
//! indistinguishable from a detector that always says "parity". Every other
//! test exists to keep that one honest — that the harness really reads the
//! store, really writes nothing, and really passes when nothing changed.

use std::path::Path;

use cas::retrieval_parity::{
    self, Baseline, ParityContext, QuerySet, RegressionKind, store_ro::ReadOnlyMemoryDb,
};
use cas_store::{SqliteStore, Store};
use cas_types::{Entry, EntryType, MemoryTier, Scope};
use tempfile::TempDir;

const QUERY_SET: &str = r#"
version = 1
default_limit = 20
default_rank_tolerance = 3

[[query]]
id = "recent"
channel = "recent"

[[query]]
id = "list"
channel = "list"

[[query]]
id = "pinned"
channel = "pinned"
rank_tolerance = 0

[[query]]
id = "helpful"
channel = "helpful"

[[query]]
id = "type-learning"
channel = "by_type"
query = "learning"

[[query]]
id = "type-context"
channel = "by_type"
query = "context"

[[query]]
id = "type-preference"
channel = "by_type"
query = "preference"

[[query]]
id = "type-observation"
channel = "by_type"
query = "observation"

[[query]]
id = "tier-working"
channel = "by_tier"
query = "working"

[[query]]
id = "tier-in-context"
channel = "by_tier"
query = "in-context"

[[query]]
id = "tier-cold"
channel = "by_tier"
query = "cold"

[[query]]
id = "tier-archive"
channel = "by_tier"
query = "archive"

[[query]]
id = "tag-rust"
channel = "by_tag"
query = "rust"

[[query]]
id = "session-merge"
channel = "session_merge"
limit = 50

[[query]]
id = "global-list"
channel = "global_list"

[[query]]
id = "search-anything"
channel = "search"
query = "sqlite migration"
"#;

/// The fixture string this suite plants in the store and then excludes.
const FIXTURE_CONTENT: &str = "Test memory from MCP protocol test";

fn entry(id: &str, content: &str, ty: EntryType, tier: MemoryTier, tags: &[&str]) -> Entry {
    let mut e = Entry::with_scope(id.to_string(), content.to_string(), Scope::Project);
    e.entry_type = ty;
    e.memory_tier = tier;
    e.tags = tags.iter().map(|t| t.to_string()).collect();
    e.title = Some(format!("title for {id}"));
    e
}

/// Seed a store covering every entry type and every tier, so the coverage
/// check in `capture` has something real to validate against.
fn seed_store(dir: &Path) -> Vec<String> {
    let store = SqliteStore::open(dir).expect("open store");
    store.init().expect("init store");

    let entries = vec![
        entry(
            "p-2026-01-01-001",
            "sqlite migration notes for the memory store",
            EntryType::Learning,
            MemoryTier::Working,
            &["rust", "sqlite"],
        ),
        entry(
            "p-2026-01-01-002",
            "always injected project convention about rust formatting",
            EntryType::Context,
            MemoryTier::InContext,
            &["rust"],
        ),
        entry(
            "p-2026-01-01-003",
            "the user prefers terse output from every command",
            EntryType::Preference,
            MemoryTier::Cold,
            &["ux"],
        ),
        entry(
            "p-2026-01-01-004",
            "observed a flaky test during the sqlite migration run",
            EntryType::Observation,
            MemoryTier::Archive,
            &["sqlite", "flaky"],
        ),
        entry(
            "p-2026-01-01-005",
            "second learning so orderings have something to order",
            EntryType::Learning,
            MemoryTier::Working,
            &["rust"],
        ),
    ];

    let ids = entries.iter().map(|e| e.id.clone()).collect();
    for e in entries {
        store.add(&e).expect("add entry");
    }
    ids
}

struct Harness {
    _dir: TempDir,
    ctx: ParityContext,
    set: QuerySet,
    ids: Vec<String>,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let ids = seed_store(dir.path());
    Harness {
        ctx: ParityContext::new(dir.path()),
        set: QuerySet::parse(QUERY_SET).expect("query set parses"),
        ids,
        _dir: dir,
    }
}

fn capture(h: &Harness) -> Baseline {
    retrieval_parity::capture(&h.ctx, &h.set, "2026-01-01T00:00:00Z".into(), false)
        .expect("capture should succeed on a fully covered corpus")
}

/// Delete one memory directly, simulating the migration losing it.
fn delete_entry(dir: &Path, id: &str) {
    let conn = rusqlite::Connection::open(dir.join("cas.db")).expect("open rw");
    let n = conn
        .execute("DELETE FROM entries WHERE id = ?1", [id])
        .expect("delete");
    assert_eq!(n, 1, "test setup must delete exactly one row");
}

// --------------------------------------------------------------------------
// Acceptance criterion 1 + 3: capture then replay against an unchanged store.
// --------------------------------------------------------------------------

#[test]
fn replay_against_an_unchanged_store_reports_full_parity() {
    let h = harness();
    let baseline = capture(&h);
    assert!(
        baseline.results.iter().any(|r| !r.hits.is_empty()),
        "baseline must actually contain hits, or parity is vacuous"
    );

    let report = retrieval_parity::replay(&h.ctx, &h.set, &baseline, 3).expect("replay");
    assert!(
        report.passed(),
        "unchanged store must report parity:\n{}",
        report.render()
    );
    assert_eq!(report.total_regressions(), 0);
}

#[test]
fn capture_is_deterministic() {
    let h = harness();
    let a = capture(&h);
    let b = capture(&h);
    assert_eq!(
        serde_json::to_value(&a.results).unwrap(),
        serde_json::to_value(&b.results).unwrap(),
        "two captures of the same store must be byte-identical, otherwise \
         every replay would report phantom regressions"
    );
}

// --------------------------------------------------------------------------
// Acceptance criterion 4: the failure path is proven, not assumed.
// --------------------------------------------------------------------------

#[test]
fn replay_detects_a_deleted_memory() {
    let h = harness();
    let baseline = capture(&h);

    let victim = &h.ids[0];
    let victim_fp = baseline
        .results
        .iter()
        .flat_map(|r| &r.hits)
        .find(|hit| &hit.id == victim)
        .map(|hit| hit.fp.clone())
        .expect("the victim must appear in the baseline for this test to mean anything");

    delete_entry(&h.ctx.cas_dir, victim);

    let report = retrieval_parity::replay(&h.ctx, &h.set, &baseline, 3).expect("replay");

    assert!(
        !report.passed(),
        "deleting a memory must fail the parity check:\n{}",
        report.render()
    );

    let regressions: Vec<_> = report
        .cases
        .iter()
        .flat_map(|c| &c.regressions)
        .filter(|r| r.kind == RegressionKind::MissingHit)
        .collect();
    assert!(
        !regressions.is_empty(),
        "expected missing-hit regressions:\n{}",
        report.render()
    );

    // "Exactly that regression": every missing hit must be the deleted memory
    // and nothing else. A detector that flags collateral damage is as useless
    // as one that flags nothing.
    for r in &regressions {
        assert_eq!(
            r.fp.as_deref(),
            Some(victim_fp.as_str()),
            "only the deleted memory should be reported missing, got: {}",
            r.detail
        );
    }
}

#[test]
fn replay_detects_a_rank_drop_beyond_tolerance() {
    let h = harness();
    let baseline = capture(&h);

    // Push a batch of newer memories in front of the baseline's top hits so
    // that the originals slide down the created-desc orderings.
    let store = SqliteStore::open(&h.ctx.cas_dir).expect("open");
    for i in 0..10 {
        let mut e = entry(
            &format!("p-2026-06-0{i}-100"),
            &format!("filler memory number {i}"),
            EntryType::Learning,
            MemoryTier::Working,
            &["rust"],
        );
        e.created = chrono::Utc::now();
        store.add(&e).expect("add filler");
    }
    drop(store);

    let report = retrieval_parity::replay(&h.ctx, &h.set, &baseline, 3).expect("replay");
    assert!(
        report
            .cases
            .iter()
            .flat_map(|c| &c.regressions)
            .any(|r| r.kind == RegressionKind::RankDrop),
        "10 newer entries must push originals past a tolerance of 3:\n{}",
        report.render()
    );
    assert!(
        report
            .cases
            .iter()
            .flat_map(|c| &c.regressions)
            .all(|r| r.kind != RegressionKind::MissingHit),
        "nothing was deleted, so nothing should be reported missing:\n{}",
        report.render()
    );
}

#[test]
fn a_generous_tolerance_forgives_the_same_rank_drop() {
    let h = harness();
    let baseline = capture(&h);
    let store = SqliteStore::open(&h.ctx.cas_dir).expect("open");
    for i in 0..10 {
        let mut e = entry(
            &format!("p-2026-06-0{i}-100"),
            &format!("filler memory number {i}"),
            EntryType::Learning,
            MemoryTier::Working,
            &["rust"],
        );
        e.created = chrono::Utc::now();
        store.add(&e).expect("add filler");
    }
    drop(store);

    let report = retrieval_parity::replay(&h.ctx, &h.set, &baseline, 100).expect("replay");
    assert!(
        report
            .cases
            .iter()
            .flat_map(|c| &c.regressions)
            .all(|r| r.kind != RegressionKind::RankDrop),
        "tolerance 100 must absorb the drift:\n{}",
        report.render()
    );
}

// --------------------------------------------------------------------------
// Acceptance criterion 2: coverage of every type and tier in the corpus.
// --------------------------------------------------------------------------

#[test]
fn capture_refuses_a_query_set_with_coverage_gaps() {
    let h = harness();
    let partial = QuerySet::parse(
        "version = 1\n[[query]]\nid=\"only\"\nchannel=\"by_type\"\nquery=\"learning\"\n",
    )
    .unwrap();

    let err = retrieval_parity::capture(&h.ctx, &partial, "t".into(), false)
        .expect_err("a query set that ignores most of the corpus must not be capturable");
    let msg = err.to_string();
    assert!(msg.contains("observation"), "got: {msg}");
    assert!(msg.contains("tier"), "got: {msg}");
    assert!(
        msg.contains("--allow-uncovered"),
        "error must say the way out: {msg}"
    );

    retrieval_parity::capture(&h.ctx, &partial, "t".into(), true)
        .expect("--allow-uncovered must permit a knowingly partial baseline");
}

#[test]
fn the_committed_query_set_parses_and_covers_all_types_and_tiers() {
    let path = cas::test_paths::workspace_root().join("fixtures/retrieval-parity/queryset.toml");
    let set = QuerySet::load(&path).expect("the committed query set must be valid");

    // Every EntryType and MemoryTier variant, not just the ones this machine
    // happens to hold today — the set must stay valid as the corpus grows.
    let corpus = cas::retrieval_parity::CorpusStats {
        active_entries: 0,
        entry_types: vec![
            "learning".into(),
            "context".into(),
            "preference".into(),
            "observation".into(),
        ],
        tiers: vec![
            "in-context".into(),
            "working".into(),
            "cold".into(),
            "archive".into(),
        ],
    };
    let gaps = set.coverage_gaps(&corpus);
    assert!(gaps.is_empty(), "committed query set has gaps: {gaps:?}");
    assert!(
        set.query
            .iter()
            .any(|c| c.channel == cas::retrieval_parity::Channel::Search),
        "the committed set must exercise the BM25 search channel"
    );
}

// --------------------------------------------------------------------------
// Acceptance criterion 5: no writes, in either mode.
// --------------------------------------------------------------------------

#[test]
fn the_read_only_connection_rejects_writes() {
    let dir = tempfile::tempdir().unwrap();
    seed_store(dir.path());
    let db = ReadOnlyMemoryDb::open(dir.path()).expect("open read-only");

    // Reads work...
    assert_eq!(db.active_count().unwrap(), 5);

    // ...and the same handle physically cannot write. This is the guarantee
    // the whole harness rests on, so it is asserted rather than assumed.
    let err = db
        .exec_for_test("DELETE FROM entries")
        .expect_err("a read-only connection must refuse a DELETE");
    assert!(
        err.to_string().to_lowercase().contains("readonly"),
        "expected a SQLITE_READONLY failure, got: {err}"
    );
    assert_eq!(db.active_count().unwrap(), 5, "corpus must be untouched");
}

#[test]
fn missing_database_is_an_error_not_an_empty_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let err = match ReadOnlyMemoryDb::open(dir.path()) {
        Ok(_) => panic!("opening a nonexistent store must fail loudly, not succeed"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("no memory database"), "got: {err}");
    assert!(
        !dir.path().join("cas.db").exists(),
        "a failed open must not have created a database"
    );
}

#[test]
fn capture_and_replay_do_not_modify_the_store() {
    let h = harness();
    let db_path = h.ctx.cas_dir.join("cas.db");

    let before = std::fs::read(&db_path).expect("read db");
    let baseline = capture(&h);
    let after_capture = std::fs::read(&db_path).expect("read db");
    assert_eq!(before, after_capture, "capture must not write to cas.db");

    retrieval_parity::replay(&h.ctx, &h.set, &baseline, 3).expect("replay");
    let after_replay = std::fs::read(&db_path).expect("read db");
    assert_eq!(before, after_replay, "replay must not write to cas.db");
}

#[test]
fn a_missing_search_index_is_never_created_by_a_run() {
    let h = harness();
    assert!(
        !h.ctx.index_dir.exists(),
        "test precondition: the seeded store has no search index"
    );

    let baseline = capture(&h);
    assert!(
        !h.ctx.index_dir.exists(),
        "capture must not create the search index it is measuring"
    );

    // The search case must be recorded as unavailable, not as a legitimate
    // zero-hit result — otherwise a later run with a working index would look
    // like an improvement rather than revealing the baseline was hollow.
    let search_case = baseline
        .results
        .iter()
        .find(|r| r.id == "search-anything")
        .expect("search case present");
    assert!(
        !search_case.status.is_ok(),
        "expected the search channel to report unavailable, got {:?}",
        search_case.status
    );

    retrieval_parity::replay(&h.ctx, &h.set, &baseline, 3).expect("replay");
    assert!(
        !h.ctx.index_dir.exists(),
        "replay must not create it either"
    );
}

// --------------------------------------------------------------------------
// Baseline round-tripping.
// --------------------------------------------------------------------------

// --------------------------------------------------------------------------
// Fixture exclusion (mapping-spec §3: 994/1696 rows are five test strings).
// --------------------------------------------------------------------------

/// Seed a store that also contains fixture rows, as the live databases do.
fn seed_with_fixtures(dir: &Path) {
    seed_store(dir);
    let store = SqliteStore::open(dir).expect("open");
    for i in 0..4 {
        let e = entry(
            &format!("p-2026-02-0{i}-900"),
            FIXTURE_CONTENT,
            EntryType::Learning,
            MemoryTier::Working,
            &[],
        );
        store.add(&e).expect("add fixture");
    }
}

fn set_excluding_fixtures() -> QuerySet {
    // Prepended, not appended: in TOML a bare key after an array-of-tables
    // would bind to the last [[query]] rather than to the document.
    QuerySet::parse(&format!(
        "exclude_contents = [\"{FIXTURE_CONTENT}\"]\n{QUERY_SET}"
    ))
    .expect("query set with exclusions parses")
}

#[test]
fn excluded_fixture_content_never_reaches_a_baseline() {
    let dir = tempfile::tempdir().unwrap();
    seed_with_fixtures(dir.path());
    let ctx = ParityContext::new(dir.path());

    let with_fixtures = retrieval_parity::capture(
        &ctx,
        &QuerySet::parse(QUERY_SET).unwrap(),
        "t".into(),
        false,
    )
    .expect("capture");
    assert!(
        with_fixtures
            .results
            .iter()
            .flat_map(|r| &r.hits)
            .any(|h| h.label.contains("900")),
        "test precondition: fixtures are visible when not excluded"
    );

    let excluded = retrieval_parity::capture(&ctx, &set_excluding_fixtures(), "t".into(), false)
        .expect("capture");
    let fixture_fp = cas::retrieval_parity::fingerprint(FIXTURE_CONTENT);
    assert!(
        excluded
            .results
            .iter()
            .flat_map(|r| &r.hits)
            .all(|h| h.fp != fixture_fp),
        "excluded content must not appear in any channel's hits"
    );
}

#[test]
fn exclusion_is_fingerprint_based_not_literal() {
    let dir = tempfile::tempdir().unwrap();
    seed_store(dir.path());
    let store = SqliteStore::open(dir.path()).unwrap();
    // Same content, different formatting — normalization must still catch it.
    store
        .add(&entry(
            "p-2026-03-01-001",
            "  TEST memory   from MCP\nprotocol test ",
            EntryType::Learning,
            MemoryTier::Working,
            &[],
        ))
        .unwrap();
    drop(store);

    let baseline = retrieval_parity::capture(
        &ParityContext::new(dir.path()),
        &set_excluding_fixtures(),
        "t".into(),
        false,
    )
    .expect("capture");
    let fixture_fp = cas::retrieval_parity::fingerprint(FIXTURE_CONTENT);
    assert!(
        baseline
            .results
            .iter()
            .flat_map(|r| &r.hits)
            .all(|h| h.fp != fixture_fp),
        "a whitespace/case variant of an excluded string must also be excluded"
    );
}

#[test]
fn excluded_rows_do_not_shift_the_ranks_of_real_rows() {
    // If fixtures kept their slots, removing them during migration would read
    // as a mass rank improvement and adding them as a mass regression.
    let clean = tempfile::tempdir().unwrap();
    seed_store(clean.path());
    let polluted = tempfile::tempdir().unwrap();
    seed_with_fixtures(polluted.path());

    let set = set_excluding_fixtures();
    let a = retrieval_parity::capture(&ParityContext::new(clean.path()), &set, "t".into(), false)
        .unwrap();
    let b = retrieval_parity::capture(
        &ParityContext::new(polluted.path()),
        &set,
        "t".into(),
        false,
    )
    .unwrap();

    let ranks = |base: &Baseline, case: &str| -> Vec<(String, usize)> {
        base.results
            .iter()
            .find(|r| r.id == case)
            .unwrap()
            .hits
            .iter()
            .map(|h| (h.fp.clone(), h.rank))
            .collect()
    };
    for case in ["list", "recent", "type-learning", "session-merge"] {
        assert_eq!(
            ranks(&a, case),
            ranks(&b, case),
            "case '{case}': fixture rows must not perturb real rows' ranks"
        );
    }
}

// --------------------------------------------------------------------------
// SessionStart merge path (inventory §3.1).
// --------------------------------------------------------------------------

#[test]
fn session_merge_prefers_project_over_global_on_the_stripped_id() {
    let project = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    seed_store(project.path());

    let gstore = SqliteStore::open(global.path()).unwrap();
    gstore.init().unwrap();
    // g-2026-01-01-001 collides with the project's p-2026-01-01-001 once the
    // scope prefix is stripped, so the project row must win and the global
    // one must not appear.
    gstore
        .add(&entry(
            "g-2026-01-01-001",
            "GLOBAL duplicate that must lose to the project row",
            EntryType::Learning,
            MemoryTier::Working,
            &[],
        ))
        .unwrap();
    gstore
        .add(&entry(
            "g-2026-01-01-777",
            "global-only memory that must be merged in",
            EntryType::Learning,
            MemoryTier::Working,
            &[],
        ))
        .unwrap();
    drop(gstore);

    let ctx = ParityContext::new(project.path()).with_global(Some(global.path().to_path_buf()));
    let baseline = retrieval_parity::capture(
        &ctx,
        &QuerySet::parse(QUERY_SET).unwrap(),
        "t".into(),
        false,
    )
    .expect("capture");

    let merged = baseline
        .results
        .iter()
        .find(|r| r.id == "session-merge")
        .expect("session-merge case");

    assert!(
        merged.hits.iter().any(|h| h.label.contains("777")),
        "global-only rows must be merged in: {:?}",
        merged.hits.iter().map(|h| &h.label).collect::<Vec<_>>()
    );
    assert!(
        merged
            .hits
            .iter()
            .all(|h| !h.fp.eq(&cas::retrieval_parity::fingerprint(
                "GLOBAL duplicate that must lose to the project row"
            ))),
        "the global row whose stripped id collides with a project row must lose"
    );
    assert!(
        merged.hits.iter().any(|h| h.id == "p-2026-01-01-001"),
        "the winning project row must be the one recorded"
    );
}

#[test]
fn session_merge_includes_archive_tier_rows() {
    // store_list applies no tier filter: archive-tier rows are live and are
    // still injected into every session. A harness that filtered them would
    // bless their loss.
    let h = harness();
    let baseline = capture(&h);
    let merged = baseline
        .results
        .iter()
        .find(|r| r.id == "session-merge")
        .expect("session-merge case");
    assert!(
        merged.hits.iter().any(|h| h.tier == "archive"),
        "archive-tier rows must appear in the merge: {:?}",
        merged.hits.iter().map(|h| &h.tier).collect::<Vec<_>>()
    );
    assert!(
        merged.hits.iter().any(|h| h.tier == "in-context"),
        "and so must in-context rows"
    );
}

#[test]
fn session_merge_without_a_global_store_is_project_only() {
    let h = harness();
    assert!(h.ctx.global_cas_dir.is_none(), "precondition");
    let baseline = capture(&h);
    let merged = baseline
        .results
        .iter()
        .find(|r| r.id == "session-merge")
        .unwrap();
    assert_eq!(
        merged.hits.len(),
        5,
        "all five seeded project rows, no more"
    );
}

#[test]
fn a_nonexistent_global_store_is_never_claimed_as_merged() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ParityContext::new(dir.path())
        .with_global(Some(Path::new("/nonexistent/cas").to_path_buf()));
    assert!(
        ctx.global_cas_dir.is_none(),
        "a global path with no cas.db must not be claimed as merged"
    );
    let reason = ctx
        .global_unavailable
        .as_deref()
        .expect("and it must not be dropped silently either");
    assert!(
        reason.contains("/nonexistent/cas"),
        "the reason must name the path that failed: {reason}"
    );
}

#[test]
fn a_requested_but_missing_global_store_makes_session_merge_unavailable() {
    // The bug this guards (cas-96ae): with_global used to filter a cas.db-less
    // path away silently, so session_merge reported Ok with project-only hits
    // and every run went green while the global tier was never measured.
    let h = harness();
    let ctx = ParityContext::new(&h.ctx.cas_dir)
        .with_global(Some(Path::new("/nonexistent/cas").to_path_buf()));
    let baseline =
        retrieval_parity::capture(&ctx, &h.set, "t".into(), false).expect("capture must still run");

    let merged = baseline
        .results
        .iter()
        .find(|r| r.id == "session-merge")
        .expect("session-merge case");

    assert!(
        !merged.status.is_ok(),
        "a merge missing its global half must be Unavailable, not Ok: {:?}",
        merged.status
    );
    assert!(
        merged.hits.is_empty(),
        "an unavailable channel records no hits that could be mistaken for a healthy merge"
    );
}

#[test]
fn an_available_global_store_leaves_session_merge_ok() {
    // The converse guard: the loud path must not fire when the store is real.
    let project = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    seed_store(project.path());
    let gstore = SqliteStore::open(global.path()).unwrap();
    gstore.init().unwrap();
    drop(gstore);

    let ctx = ParityContext::new(project.path()).with_global(Some(global.path().to_path_buf()));
    assert!(ctx.global_unavailable.is_none(), "a real store is available");

    let baseline = retrieval_parity::capture(
        &ctx,
        &QuerySet::parse(QUERY_SET).unwrap(),
        "t".into(),
        false,
    )
    .expect("capture");
    let merged = baseline
        .results
        .iter()
        .find(|r| r.id == "session-merge")
        .unwrap();
    assert!(merged.status.is_ok(), "{:?}", merged.status);
}

#[test]
fn the_global_channel_measures_the_global_store_not_the_project_one() {
    // session_merge lists project rows first and truncates, so on a real host
    // it can never surface global content. This channel reads the global store
    // directly — that is what makes the global tier measurable at all.
    let project = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    seed_store(project.path());

    let gstore = SqliteStore::open(global.path()).unwrap();
    gstore.init().unwrap();
    gstore
        .add(&entry(
            "g-2026-01-01-777",
            "global-only memory that the global channel must see",
            EntryType::Learning,
            MemoryTier::Working,
            &[],
        ))
        .unwrap();
    drop(gstore);

    let ctx = ParityContext::new(project.path()).with_global(Some(global.path().to_path_buf()));
    let baseline = retrieval_parity::capture(
        &ctx,
        &QuerySet::parse(QUERY_SET).unwrap(),
        "t".into(),
        false,
    )
    .expect("capture");

    let g = baseline
        .results
        .iter()
        .find(|r| r.id == "global-list")
        .expect("global-list case");
    assert!(g.status.is_ok(), "{:?}", g.status);
    assert_eq!(g.hits.len(), 1, "exactly the one global row");
    assert_eq!(g.hits[0].id, "g-2026-01-01-777");
    assert!(
        !g.hits.iter().any(|h| h.id.starts_with("p-")),
        "the global channel must not be reading the project store"
    );
}

#[test]
fn the_global_channel_is_unavailable_rather_than_empty_without_a_store() {
    // Zero hits would be indistinguishable from "the global store is empty",
    // which is exactly the disguise the original bug wore.
    let h = harness();
    assert!(h.ctx.global_cas_dir.is_none(), "precondition");
    let baseline = capture(&h);
    let g = baseline
        .results
        .iter()
        .find(|r| r.id == "global-list")
        .unwrap();
    assert!(
        !g.status.is_ok(),
        "no global store must read as UNAVAILABLE, not as an empty result"
    );
    assert!(g.hits.is_empty());
}

#[test]
fn baseline_round_trips_through_disk() {
    let h = harness();
    let baseline = capture(&h);
    let out = h.ctx.cas_dir.join("nested/baseline.json");

    retrieval_parity::save_baseline(&out, &baseline).expect("save");
    let loaded = retrieval_parity::load_baseline(&out).expect("load");

    assert_eq!(
        serde_json::to_value(&baseline).unwrap(),
        serde_json::to_value(&loaded).unwrap()
    );

    let report = retrieval_parity::replay(&h.ctx, &h.set, &loaded, 3).expect("replay");
    assert!(report.passed(), "{}", report.render());
}

#[test]
fn a_baseline_from_a_future_format_is_refused() {
    let h = harness();
    let mut baseline = capture(&h);
    baseline.version = retrieval_parity::BASELINE_VERSION + 1;

    let err = retrieval_parity::replay(&h.ctx, &h.set, &baseline, 3)
        .expect_err("a version mismatch must not be silently compared");
    assert!(err.to_string().contains("version"), "got: {err}");
}
