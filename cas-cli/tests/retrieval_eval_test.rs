//! Labeled retrieval evaluation — layer 1 of the EPIC cas-8fac eval harness
//! (task cas-e0ed).
//!
//! The sibling `retrieval_parity_test.rs` proves retrieval did not *change*.
//! This suite proves how *good* it is: 56 hand-judged prompt-contexts replayed
//! through the two selectors that actually put memories in front of a model,
//! scored precision@5 / recall@5, gated against a committed baseline.
//!
//! The load-bearing test here is
//! [`the_gate_fires_on_a_deliberate_corpus_regression`]: a gate that has never
//! been shown to fail is indistinguishable from no gate. Every other test
//! exists to keep that one honest — that the fixture is self-contained, that
//! the harness reads the real selectors, and that an unchanged corpus is green.
//!
//! # RE-BASELINE PROCEDURE (one line)
//!
//! ```text
//! CAS_RETRIEVAL_EVAL_REBASELINE=1 cargo nextest run -p cas --test retrieval_eval_test
//! ```
//!
//! That rewrites `cas-cli/tests/data/retrieval-eval/baseline.json` from the
//! current run instead of gating on it. Commit the rewritten baseline in the
//! same commit as the change that moved the numbers, and say in the commit
//! message which selector moved and why the new number is the better one. A
//! re-baseline with no stated reason is a silently accepted regression.
//!
//! # Reading the numbers
//!
//! See the module docs on `cas::retrieval_eval` for what each metric means and
//! why `live_tiers` and `all_working` are both reported. In short: `live_tiers`
//! is the shipped end-to-end behaviour (the Helpful-Memories tier filter can
//! only see 14 of the 189 real entries), `all_working` isolates the ranking
//! function so a ranking change has a metric that can actually move.
//!
//! # This fixture ships in a public repository
//!
//! The corpus is mined from a real store that also holds client-confidential
//! material. That material was removed, not redacted, and everything that
//! remains went through a redaction pass (see `provenance.redaction` in the
//! fixture). [`the_fixture_carries_no_secret_or_client_confidential_shapes`]
//! keeps that true for any future edit — treat a failure there as a disclosure
//! incident, not a broken test.

use std::collections::HashSet;

use cas::retrieval_eval::{
    self, Baseline, EvalCorpus, EvalFixture, REBASELINE_ENV, REGRESSION_TOLERANCE,
    SELECTOR_AMBIENT_CANDIDATES, SELECTOR_AMBIENT_PACKET, SELECTOR_HELPFUL_MEMORIES, TierMode,
};

fn fixture() -> EvalFixture {
    EvalFixture::load(&EvalFixture::committed_path()).expect("committed fixture must load")
}

// --------------------------------------------------------------------------
// Acceptance criterion 1: the fixture is real, labeled, and self-contained.
// --------------------------------------------------------------------------

#[test]
fn the_committed_fixture_is_well_formed_and_self_contained() {
    let fixture = fixture();
    assert!(
        fixture.cases.len() >= 50,
        "the brief asks for ~50 labeled pairs, got {}",
        fixture.cases.len()
    );
    assert!(
        fixture.entries.len() >= 100,
        "a corpus small enough to fit in the @5 window makes precision vacuous, got {}",
        fixture.entries.len()
    );

    let ids: HashSet<&str> = fixture.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        fixture.entries.len(),
        "duplicate entry ids in the fixture corpus"
    );

    for case in &fixture.cases {
        assert!(
            !case.relevant.is_empty(),
            "{}: a case with no relevant entry has an undefined recall",
            case.case_id
        );
        assert!(
            !case.judged_by.trim().is_empty() && !case.judged_at.trim().is_empty(),
            "{}: every judgment must carry judged_by and judged_at",
            case.case_id
        );
        assert!(
            !case.user_prompt.trim().is_empty(),
            "{}: a case needs a prompt context",
            case.case_id
        );
        // Self-containment: no label may point outside the snapshotted corpus,
        // or the metric would silently depend on a store this test never reads.
        for id in case.relevant.iter().chain(case.ambiguous.iter()) {
            assert!(
                ids.contains(id.as_str()),
                "{}: label {id} is not in the snapshotted corpus",
                case.case_id
            );
        }
        let strict: HashSet<&str> = case.relevant.iter().map(String::as_str).collect();
        for id in &case.ambiguous {
            assert!(
                !strict.contains(id.as_str()),
                "{}: {id} is labeled both relevant and ambiguous",
                case.case_id
            );
        }
    }

    // Distractors are what make precision mean anything: if every entry were
    // relevant to something, a selector could not be wrong.
    let labeled: HashSet<&str> = fixture
        .cases
        .iter()
        .flat_map(|c| c.relevant.iter().chain(c.ambiguous.iter()))
        .map(String::as_str)
        .collect();
    assert!(
        ids.len() - labeled.len() >= 20,
        "corpus needs a meaningful pool of never-relevant distractors, got {}",
        ids.len() - labeled.len()
    );
}

#[test]
fn the_fixture_carries_no_secret_or_client_confidential_shapes() {
    // This file is committed to a public repository and was mined from a store
    // that also holds a client's accounting history. Those memories and the
    // tasks that referenced them were removed; everything left went through a
    // redaction pass. This test is the standing guard on that pass — a hit here
    // means confidential material is about to be published, so treat it as a
    // disclosure incident rather than a formatting nit.
    let raw = std::fs::read_to_string(EvalFixture::committed_path()).expect("read fixture");

    let forbidden: &[(&str, &str)] = &[
        ("IPv4 address", r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
        ("currency amount", r"\$[\d,]{3,}"),
        ("email address", r"[\w.+-]+@[\w-]+\.[\w.]{2,}"),
        ("long numeric id", r"\b\d{6,}\b"),
        ("tailnet hostname", r"\.ts\.net"),
        ("Slack channel id", r"\bC0[A-Z0-9]{7,}\b"),
        (
            "credential-shaped string",
            r"(gh[pousr]_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9]{20,}|xox[bpa]-)",
        ),
        ("operator home path", r"/home/pippenz"),
        (
            "client-domain term",
            r"\b(Roark|Cirrus|Rearden|SugarTree|Moultrie|Renovo|QBO|Form 1065|Form 8825)\b",
        ),
    ];

    for (label, pattern) in forbidden {
        let re = regex::Regex::new(pattern).expect("pattern compiles");
        if let Some(hit) = re.find(&raw) {
            let start = raw[..hit.start()].rfind('\n').map_or(0, |i| i + 1);
            let end = raw[hit.end()..]
                .find('\n')
                .map_or(raw.len(), |i| hit.end() + i);
            panic!(
                "fixture contains a {label} — do not commit this.\n  match: {}\n  line: {}",
                hit.as_str(),
                &raw[start..end.min(start + 300)]
            );
        }
    }
}

#[test]
fn the_fixture_carries_the_real_tier_skew_it_claims_to_measure() {
    // The whole point of reporting two tier modes is that the live corpus is
    // almost entirely archive-tier. If a future fixture edit lifted the tiers,
    // the `live_tiers` numbers would silently stop describing production.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize(&fixture, dir.path(), TierMode::Live).expect("seed");
    assert!(
        corpus.active_entries() < fixture.entries.len() / 4,
        "fixture no longer reflects the live archive-tier skew: {} of {} entries are active",
        corpus.active_entries(),
        fixture.entries.len()
    );

    let all_working_dir = tempfile::tempdir().expect("tempdir");
    let lifted =
        EvalCorpus::materialize(&fixture, all_working_dir.path(), TierMode::AllWorking).expect("seed");
    assert_eq!(
        lifted.active_entries(),
        fixture.entries.len(),
        "all_working must neutralise the tier filter completely"
    );
}

// --------------------------------------------------------------------------
// The measured findings, pinned so a future change has to notice them.
// --------------------------------------------------------------------------

#[test]
fn helpful_memories_returns_the_same_ranking_for_every_prompt_context() {
    // This is the headline layer-1 finding, and the reason the P@5 row for
    // `helpful_memories` is near zero: `BasicContextScorer::score_entries`
    // takes `_context: &ContextQuery` and ignores it entirely
    // (crates/cas-core/src/hooks/context/mod.rs:193). The default SessionStart
    // ranking is therefore a function of the corpus alone — type weight,
    // feedback, age decay, importance, stability — and cannot vary with what
    // the session is about.
    //
    // When P6b wires query relevance into this selector, this test SHOULD
    // fail. Replace it with the assertion that the ranking *does* vary, and
    // re-baseline; do not delete it.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize(&fixture, dir.path(), TierMode::AllWorking).expect("seed");

    let mut rankings = fixture
        .cases
        .iter()
        .map(|case| retrieval_eval::helpful_memories_ranking(&corpus, case).expect("rank"));
    let first = rankings.next().expect("at least one case");
    assert!(!first.is_empty(), "the selector must return something");
    for (case, ranking) in fixture.cases.iter().skip(1).zip(rankings) {
        assert_eq!(
            ranking, first,
            "{}: Helpful Memories varied with the prompt context — if that is \
             now intended, update this test and re-baseline",
            case.case_id
        );
    }
}

#[test]
fn the_two_tier_modes_agree_today_and_the_harness_says_why() {
    // Both tier rows in the committed baseline are currently identical, and a
    // reader is right to suspect a copy-paste. They are not:
    //
    // * The ambient selectors filter on `archived = 0` only — no tier
    //   predicate exists on that path, so the modes agree by construction.
    // * Helpful Memories sorts high-importance preferences (importance >= 0.9)
    //   ahead of everything else, and the five that win are recent enough that
    //   their age decay beats every archive-tier preference of equal
    //   importance. Lifting the tier filter therefore adds 169 candidates that
    //   all score below the incumbents.
    //
    // The mode still earns its place: it is the only way a future ranking
    // change can show up as movement rather than be swallowed by the tier
    // filter. This test asserts the *reason*, so if the coincidence ever ends
    // the baseline diff is explained rather than mysterious.
    let fixture = fixture();
    let live = tempfile::tempdir().expect("tempdir");
    let working = tempfile::tempdir().expect("tempdir");
    let (metrics, _) = retrieval_eval::run_all(&fixture, live.path(), working.path()).expect("run");

    for selector in [
        SELECTOR_HELPFUL_MEMORIES,
        SELECTOR_AMBIENT_PACKET,
        SELECTOR_AMBIENT_CANDIDATES,
    ] {
        let live_row = metrics
            .iter()
            .find(|m| m.selector == selector && m.tier_mode == TierMode::Live.as_str())
            .expect("live row");
        let working_row = metrics
            .iter()
            .find(|m| m.selector == selector && m.tier_mode == TierMode::AllWorking.as_str())
            .expect("all_working row");
        assert_eq!(
            (live_row.precision_at_5, live_row.recall_at_5),
            (working_row.precision_at_5, working_row.recall_at_5),
            "{selector}: the tier modes diverged. That is not a failure — it means a \
             ranking or tier change landed. Re-baseline and record which."
        );
    }

    // And the top-5 Helpful Memories really are the high-importance
    // preferences the comment above blames, not some other coincidence.
    let fresh = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize(&fixture, fresh.path(), TierMode::Live).expect("seed");
    let top = retrieval_eval::helpful_memories_ranking(&corpus, &fixture.cases[0]).expect("rank");
    let by_id: std::collections::HashMap<&str, &_> = fixture
        .entries
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    for id in &top {
        let entry = by_id.get(id.as_str()).expect("surfaced id must be in fixture");
        assert_eq!(entry.entry_type, "preference", "{id} is not a preference");
        assert!(
            entry.importance >= 0.9,
            "{id} importance {} is below the high-importance threshold",
            entry.importance
        );
    }
}

// --------------------------------------------------------------------------
// Acceptance criterion 2 + 3: report the metrics, gate on the baseline.
// --------------------------------------------------------------------------

#[test]
fn harness_reports_precision_and_recall_and_holds_the_committed_baseline() {
    let fixture = fixture();
    let live = tempfile::tempdir().expect("tempdir");
    let all_working = tempfile::tempdir().expect("tempdir");

    let (metrics, _details) =
        retrieval_eval::run_all(&fixture, live.path(), all_working.path()).expect("run");

    println!(
        "\nretrieval eval — fixture {} ({} cases, {} entries)\n{}",
        fixture.fixture_id,
        fixture.cases.len(),
        fixture.entries.len(),
        retrieval_eval::render_table(&metrics)
    );

    let baseline_path = EvalFixture::committed_baseline_path();

    if std::env::var(REBASELINE_ENV).is_ok() {
        let baseline = Baseline {
            version: retrieval_eval::BASELINE_VERSION,
            fixture_id: fixture.fixture_id.clone(),
            captured_at: chrono::Utc::now().date_naive().to_string(),
            note: "Captured by CAS_RETRIEVAL_EVAL_REBASELINE=1. See the test module header \
                   for the re-baseline procedure and what each metric means."
                .to_string(),
            selectors: metrics.clone(),
        };
        baseline.save(&baseline_path).expect("write baseline");
        println!("re-baselined {}", baseline_path.display());
        return;
    }

    let baseline = Baseline::load(&baseline_path).expect("committed baseline must load");
    assert_eq!(
        baseline.fixture_id, fixture.fixture_id,
        "baseline was captured against a different fixture; re-baseline deliberately"
    );

    // Every selector the baseline knows about must still be measured, and the
    // committed numbers must be non-trivial for at least one selector — a
    // baseline of all zeroes would pass any gate.
    assert!(
        baseline
            .selectors
            .iter()
            .any(|m| m.precision_at_5 > 0.0 && m.recall_at_5 > 0.0),
        "a baseline with no positive metric cannot detect a regression"
    );

    let (regressions, missing) = retrieval_eval::compare(&baseline, &metrics, REGRESSION_TOLERANCE);
    assert!(
        missing.is_empty(),
        "baselined selectors were not measured: {missing:?}"
    );
    assert!(
        regressions.is_empty(),
        "retrieval regressed beyond the {:.0}% tolerance:\n{}\n\nIf this is intended, \
         re-baseline deliberately (see the test module header).",
        REGRESSION_TOLERANCE * 100.0,
        regressions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn two_runs_of_the_harness_produce_identical_metrics() {
    // Without this, every gate failure would be indistinguishable from noise.
    let fixture = fixture();
    let run = || {
        let live = tempfile::tempdir().expect("tempdir");
        let working = tempfile::tempdir().expect("tempdir");
        let (metrics, _) =
            retrieval_eval::run_all(&fixture, live.path(), working.path()).expect("run");
        metrics
    };
    assert_eq!(run(), run(), "the harness must be deterministic");
}

// --------------------------------------------------------------------------
// Acceptance criterion 3: the failure path is proven, not assumed.
// --------------------------------------------------------------------------

#[test]
fn the_gate_fires_on_a_deliberate_corpus_regression() {
    let fixture = fixture();

    // Baseline: the real corpus, captured in-test so this proof does not depend
    // on the committed numbers staying still.
    let live = tempfile::tempdir().expect("tempdir");
    let working = tempfile::tempdir().expect("tempdir");
    let (clean, _) = retrieval_eval::run_all(&fixture, live.path(), working.path()).expect("run");
    let reference = Baseline {
        version: retrieval_eval::BASELINE_VERSION,
        fixture_id: fixture.fixture_id.clone(),
        captured_at: "in-test".to_string(),
        note: "in-test reference".to_string(),
        selectors: clean.clone(),
    };

    // The perturbation is a real data regression, not a doctored number: half
    // the labeled-relevant memories are archived out of the corpus. Both
    // selectors filter on `archived = 0`, so this is exactly what losing those
    // rows to a bad migration would look like.
    let victims: Vec<String> = {
        let mut ids: Vec<String> = fixture
            .cases
            .iter()
            .flat_map(|c| c.relevant.iter().cloned())
            .collect();
        ids.sort();
        ids.dedup();
        ids.into_iter().step_by(2).collect()
    };
    assert!(victims.len() > 20, "perturbation must be substantial");

    let damaged_live = tempfile::tempdir().expect("tempdir");
    let damaged_working = tempfile::tempdir().expect("tempdir");
    let mut damaged = Vec::new();
    for (mode, dir) in [
        (TierMode::Live, damaged_live.path()),
        (TierMode::AllWorking, damaged_working.path()),
    ] {
        let corpus = EvalCorpus::materialize(&fixture, dir, mode).expect("seed");
        archive_entries(dir, &victims);

        let (m, _) = retrieval_eval::score(SELECTOR_HELPFUL_MEMORIES, mode, &fixture.cases, |c| {
            retrieval_eval::helpful_memories_ranking(&corpus, c).unwrap_or_default()
        });
        damaged.push(m);
        let (m, _) = retrieval_eval::score(SELECTOR_AMBIENT_PACKET, mode, &fixture.cases, |c| {
            retrieval_eval::ambient_packet_ranking(&corpus, c)
        });
        damaged.push(m);
        let (m, _) = retrieval_eval::score(SELECTOR_AMBIENT_CANDIDATES, mode, &fixture.cases, |c| {
            retrieval_eval::ambient_candidate_ranking(&corpus, c)
        });
        damaged.push(m);
    }

    let (regressions, missing) =
        retrieval_eval::compare(&reference, &damaged, REGRESSION_TOLERANCE);
    assert!(missing.is_empty(), "coverage must be unchanged: {missing:?}");
    assert!(
        !regressions.is_empty(),
        "archiving half the labeled-relevant corpus must trip the gate; it did not.\n\
         clean:\n{}\ndamaged:\n{}",
        retrieval_eval::render_table(&clean),
        retrieval_eval::render_table(&damaged)
    );

    // And it must be the ambient selectors that report it: they are the ones
    // whose ranking actually depends on the query, so a corpus loss on the
    // relevant side has to show up there.
    assert!(
        regressions
            .iter()
            .any(|r| r.key.starts_with(SELECTOR_AMBIENT_CANDIDATES)),
        "expected the query-dependent selector to notice:\n{}",
        regressions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn an_unchanged_corpus_does_not_trip_the_gate() {
    // The converse guard: a gate that always fires is as useless as one that
    // never does.
    let fixture = fixture();
    let live = tempfile::tempdir().expect("tempdir");
    let working = tempfile::tempdir().expect("tempdir");
    let (metrics, _) = retrieval_eval::run_all(&fixture, live.path(), working.path()).expect("run");
    let reference = Baseline {
        version: retrieval_eval::BASELINE_VERSION,
        fixture_id: fixture.fixture_id.clone(),
        captured_at: "in-test".to_string(),
        note: "in-test reference".to_string(),
        selectors: metrics.clone(),
    };
    let (regressions, missing) =
        retrieval_eval::compare(&reference, &metrics, REGRESSION_TOLERANCE);
    assert!(regressions.is_empty(), "{regressions:?}");
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn a_tolerable_drift_passes_and_one_step_past_the_tolerance_fails() {
    // Pin the tolerance arithmetic itself, so "10%" cannot quietly become
    // "any drop" or "no drop".
    let base = metrics_stub(0.5, 0.4);
    let reference = Baseline {
        version: retrieval_eval::BASELINE_VERSION,
        fixture_id: "stub".to_string(),
        captured_at: "in-test".to_string(),
        note: String::new(),
        selectors: vec![base.clone()],
    };

    // 9% down on precision: inside tolerance.
    let tolerable = vec![metrics_stub(0.455, 0.4)];
    let (regressions, _) = retrieval_eval::compare(&reference, &tolerable, REGRESSION_TOLERANCE);
    assert!(regressions.is_empty(), "9% drop must pass: {regressions:?}");

    // 12% down on precision: outside tolerance.
    let intolerable = vec![metrics_stub(0.44, 0.4)];
    let (regressions, _) = retrieval_eval::compare(&reference, &intolerable, REGRESSION_TOLERANCE);
    assert_eq!(regressions.len(), 1, "{regressions:?}");
    assert_eq!(regressions[0].metric, "precision@5");

    // An improvement is never a regression.
    let better = vec![metrics_stub(0.9, 0.9)];
    let (regressions, _) = retrieval_eval::compare(&reference, &better, REGRESSION_TOLERANCE);
    assert!(regressions.is_empty(), "{regressions:?}");
}

#[test]
fn a_dropped_selector_is_reported_rather_than_skipped() {
    // The classic way a gate stops gating: the measurement disappears and the
    // comparison has nothing to complain about.
    let reference = Baseline {
        version: retrieval_eval::BASELINE_VERSION,
        fixture_id: "stub".to_string(),
        captured_at: "in-test".to_string(),
        note: String::new(),
        selectors: vec![metrics_stub(0.5, 0.4)],
    };
    let (regressions, missing) = retrieval_eval::compare(&reference, &[], REGRESSION_TOLERANCE);
    assert!(regressions.is_empty());
    assert_eq!(missing.len(), 1);
    assert!(missing[0].key.starts_with(SELECTOR_HELPFUL_MEMORIES));
}

// --------------------------------------------------------------------------
// Acceptance criterion 4: the documented re-baseline path is the real one.
// --------------------------------------------------------------------------

#[test]
fn the_documented_rebaseline_switch_is_the_one_the_harness_reads() {
    assert_eq!(
        REBASELINE_ENV, "CAS_RETRIEVAL_EVAL_REBASELINE",
        "the module header documents this exact variable; renaming it silently \
         would leave the documented procedure a no-op"
    );
    let header = include_str!("retrieval_eval_test.rs");
    assert!(
        header.contains("CAS_RETRIEVAL_EVAL_REBASELINE=1 cargo nextest run"),
        "the one-line re-baseline procedure must stay in this module header"
    );
}

// --------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------

fn metrics_stub(precision: f64, recall: f64) -> retrieval_eval::SelectorMetrics {
    retrieval_eval::SelectorMetrics {
        selector: SELECTOR_HELPFUL_MEMORIES.to_string(),
        tier_mode: TierMode::Live.as_str().to_string(),
        cases: 55,
        precision_at_5: precision,
        recall_at_5: recall,
        lenient_precision_at_5: precision,
        lenient_recall_at_5: recall,
        mean_returned: 5.0,
        cases_with_a_hit: 10,
        silent_cases: 0,
    }
}

fn archive_entries(cas_dir: &std::path::Path, ids: &[String]) {
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("open rw");
    let mut archived = 0usize;
    for id in ids {
        archived += conn
            .execute("UPDATE entries SET archived = 1 WHERE id = ?1", [id])
            .expect("archive");
    }
    assert_eq!(
        archived,
        ids.len(),
        "test setup must archive every named entry"
    );
}
