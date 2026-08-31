//! Labeled retrieval evaluation — layer 1 of the EPIC cas-8fac eval harness
//! (tasks cas-e0ed, cas-b06c).
//!
//! The sibling `retrieval_parity_test.rs` proves retrieval did not *change*.
//! This suite proves how *good* it is: 56 hand-judged prompt-contexts replayed
//! through the selectors that actually put memories in front of a model,
//! scored precision@5 / recall@5, gated against a committed baseline.
//!
//! Two tests are load-bearing:
//!
//! * [`the_gate_fires_on_a_deliberate_corpus_regression`] — a gate that has
//!   never been shown to fail is indistinguishable from no gate.
//! * [`the_fast_production_runner_matches_the_real_build_context_path`] — the
//!   baseline is produced by a runner that hoists the store and scorer opens
//!   out of the per-case loop, and that is only legitimate while it stays
//!   ranking-identical to the real `crate::hooks::build_context`.
//!
//! Every other test exists to keep those honest — that the fixture is
//! self-contained, that the harness reads the real selectors, and that an
//! unchanged corpus is green.
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
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cas::retrieval_eval::{
    self, Baseline, EXPECTED_CASE_COUNT, EvalCorpus, EvalFixture, HARNESS_RUNTIME_BUDGET,
    ProductionScorerState, QueryMode, REBASELINE_ENV, REGRESSION_TOLERANCE,
    SELECTOR_AMBIENT_CANDIDATES, SELECTOR_AMBIENT_PACKET, SELECTOR_HELPFUL_MEMORIES,
    SELECTOR_HELPFUL_MEMORIES_PRODUCTION, ScorerConfig, TierMode,
};
use sha2::{Digest, Sha256};

#[path = "../src/test_env_guard.rs"]
mod test_env_guard;
use test_env_guard::TestEnvGuard;

fn fixture() -> EvalFixture {
    EvalFixture::load(&EvalFixture::committed_path()).expect("committed fixture must load")
}

fn run(
    fixture: &EvalFixture,
) -> (
    Vec<retrieval_eval::SelectorMetrics>,
    retrieval_eval::CaseBreakdown,
) {
    retrieval_eval::run_all(fixture).expect("harness run")
}

// --------------------------------------------------------------------------
// Acceptance criterion 1: the fixture is real, labeled, and self-contained.
// --------------------------------------------------------------------------

#[test]
fn the_committed_fixture_is_well_formed_and_self_contained() {
    let fixture = fixture();
    assert_eq!(
        fixture.cases.len(),
        EXPECTED_CASE_COUNT,
        "the equivalence and budget pins cover a fixed 56-case fixture"
    );
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
    let fixture = fixture();
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

    // The public fixture was mined from a store containing operator
    // collaborators' personal memories. Keep the collaborator denylist as
    // SHA-256 digests so the guard does not publish the names it protects.
    // These digests are for known third-party first names in the pre-fix
    // public fixture. Normalize case before hashing so a lower-case mention
    // cannot bypass the guard; enumerate additional names from the source
    // store only when the redaction audit finds them.
    const PERSONAL_NAME_SHA256: &[&str] = &[
        "6700869c8ff7480e34a70a708b028700dbaa3a033b5652b903afe89f49a31456",
        "030d756286e59f22a464c36e1fbff606a795dfc70aaf0108bd86f2aa193d05f4",
    ];
    let name = regex::Regex::new(r"(?i)\b[a-z]{2,24}\b").expect("name pattern compiles");
    for entry in &fixture.entries {
        for token in name.find_iter(&format!("{} {}", entry.title, entry.body)) {
            let normalized = token.as_str().to_ascii_lowercase();
            let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
            assert!(
                !PERSONAL_NAME_SHA256.contains(&digest.as_str()),
                "fixture contains a denylisted personal name in {} — redact it",
                entry.id
            );
        }
    }
    for case in &fixture.cases {
        let public_fields = std::iter::once(case.task_title.as_str())
            .chain(case.task_labels.iter().map(String::as_str))
            .chain(std::iter::once(case.user_prompt.as_str()));
        for field in public_fields {
            for token in name.find_iter(field) {
                let normalized = token.as_str().to_ascii_lowercase();
                let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
                assert!(
                    !PERSONAL_NAME_SHA256.contains(&digest.as_str()),
                    "fixture contains a denylisted personal name in case {} — redact it",
                    case.case_id
                );
            }
        }
    }

    // A quoted sentence introduced as "exact words" is a named-person
    // attribution shape in the pre-fix third-party entry. Keep this explicit
    // shape guard alongside the hashed-name guard so redacting only the name
    // cannot leave the person's words in the public fixture.
    let attributed_quote = regex::Regex::new(r#"(?i)exact words\s*:\s*"[^"]+""#)
        .expect("attributed quote pattern compiles");
    assert!(
        fixture
            .entries
            .iter()
            .all(|entry| !attributed_quote.is_match(&entry.body)),
        "fixture contains a quoted sentence attributed as exact words; paraphrase it"
    );
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
// cas-b06c: the PRODUCTION Helpful-Memories path.
//
// cas-e0ed's `helpful_memories` selector passes `ContextStores::empty()`, so
// `build_start.rs` falls back to `BasicContextScorer`. Production does not:
// `cas-cli/src/hooks/context.rs` opens `HybridContextScorer::open_with_graph`
// and passes it as `entry_scorer`. Everything below measures that real path
// and keeps the Basic selector as the labelled fallback control.
// --------------------------------------------------------------------------

#[test]
fn the_production_selector_is_measured_and_gated_in_every_mode() {
    let fixture = fixture();
    let (metrics, _) = run(&fixture);

    for tier in [TierMode::Live, TierMode::AllWorking] {
        for query_mode in QueryMode::ALL {
            let row = metrics
                .iter()
                .find(|m| {
                    m.selector == SELECTOR_HELPFUL_MEMORIES_PRODUCTION
                        && m.tier_mode == tier.as_str()
                        && m.query_mode == query_mode.as_str()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no production row for {}/{}",
                        tier.as_str(),
                        query_mode.as_str()
                    )
                });
            assert_eq!(row.cases, fixture.cases.len());
        }
    }

    // Every measured row must be in the committed baseline, or the gate does
    // not actually cover the new selector.
    let baseline =
        Baseline::load(&EvalFixture::committed_baseline_path()).expect("baseline loads");
    for row in &metrics {
        assert!(
            baseline.get(&row.key()).is_some(),
            "{} is measured but not baselined — it would not be gated",
            row.key()
        );
    }
}

#[test]
fn the_production_path_ranks_differently_from_the_basic_fallback() {
    // AC3. If this ever stops being true, the production selector has silently
    // degraded to the fallback and the whole point of cas-b06c is gone.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize_with_index(&fixture, dir.path(), TierMode::AllWorking)
        .expect("seed + index");
    assert!(
        corpus.has_search_index(),
        "precondition: the fixture corpus must carry a real Tantivy index"
    );

    let runner = retrieval_eval::ProductionRunner::open(&corpus).expect("runner");
    let mut differing = Vec::new();
    for case in &fixture.cases {
        let basic = retrieval_eval::helpful_memories_ranking(&corpus, case).expect("basic");
        let production = runner.rank(case, QueryMode::SeededTask).expect("production");
        if basic != production {
            differing.push((case.case_id.clone(), basic, production));
        }
    }

    assert!(
        !differing.is_empty(),
        "the production path produced the Basic ranking for all {} cases — \
         either the index is not being used or the scorer is not wired",
        fixture.cases.len()
    );
    println!(
        "production differs from Basic on {}/{} cases; first: {:?}",
        differing.len(),
        fixture.cases.len(),
        differing.first()
    );
}

#[test]
fn a_fresh_session_is_query_aware_and_the_number_says_so() {
    // This test was born asserting the defect: a fresh session had no
    // in-progress task and no session_files.json, `has_content()` excluded cwd,
    // and production collapsed to ONE ranking for all 56 cases (cas-b06c).
    // cas-3b80 fixed that, so the assertion is re-aimed rather than deleted —
    // but at the RIGHT mode. Each of the three modes pins a different claim:
    //
    // * fresh_session (true cold start) — cwd and branch are the only content,
    //   and this fixture's 56 cases share one cwd in a non-repository temp dir.
    //   Its query is therefore identical for every case and ONE distinct
    //   ranking is the correct answer. What must be true is that it is no
    //   longer the *Basic* list: the hybrid path now runs on a constant query
    //   instead of early-returning.
    // * fresh_session_carried_prompt — the previous session's prompt is in the
    //   store, so ranking must vary case by case. This is where the cas-3b80
    //   "off 1" result lives.
    // * seeded_task — unchanged behaviour, still query-dependent.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize_with_index(&fixture, dir.path(), TierMode::AllWorking)
        .expect("seed + index");

    let fresh = distinct_rankings(&fixture, &corpus, QueryMode::FreshSession);
    let carried = distinct_rankings(&fixture, &corpus, QueryMode::FreshSessionCarriedPrompt);
    let seeded = distinct_rankings(&fixture, &corpus, QueryMode::SeededTask);
    println!(
        "distinct top-5 rankings — fresh_session: {fresh}, \
         fresh_session_carried_prompt: {carried}, seeded_task: {seeded}"
    );

    assert_eq!(
        fresh, 1,
        "every case shares one cwd and there is no branch, so a true cold start \
         necessarily issues one query; {fresh} distinct rankings means the mode \
         is picking up state it is supposed to be measured without"
    );
    assert!(
        carried > 1,
        "with the previous session's prompt carried forward, production must \
         vary with what the session is about; {carried} distinct rankings \
         across {} cases means it collapsed back to the query-blind Basic list. \
         Re-baseline only after finding out why.",
        fixture.cases.len()
    );
    assert!(
        seeded > 1,
        "with an in-progress task seeded, production must vary with the query; \
         got {seeded} distinct rankings"
    );

    // The cold start's one ranking must be the HYBRID one, not the Basic
    // fallback it used to take: that is the half of cas-3b80 the constant-query
    // mode exists to measure.
    let case = &fixture.cases[0];
    let runner = retrieval_eval::ProductionRunner::open(&corpus).expect("runner");
    let cold = runner.rank(case, QueryMode::FreshSession).expect("cold");
    let basic = retrieval_eval::helpful_memories_ranking(&corpus, case).expect("basic");
    println!("cold-start top-5: {cold:?}\nbasic top-5:      {basic:?}");
    assert_ne!(
        cold, basic,
        "a cold start still produced the Basic fallback ranking — cwd/branch \
         content is not reaching the hybrid scorer"
    );
}

#[test]
fn the_production_scorer_state_is_reported_not_guessed() {
    // The four states a real SessionStart can land in. Conflating them is how
    // "we have a hybrid scorer" turns into "we ship Basic and never notice".
    let fixture = fixture();
    let case = &fixture.cases[0];

    let indexed = tempfile::tempdir().expect("tempdir");
    let with_index = EvalCorpus::materialize_with_index(&fixture, indexed.path(), TierMode::AllWorking)
        .expect("seed + index");
    assert_eq!(
        retrieval_eval::probe_production_scorer_state(&with_index, case, QueryMode::SeededTask),
        ProductionScorerState::QueryAwareWithLexicalIndex,
        "an index plus an in-progress task is the full hybrid path"
    );
    assert_eq!(
        retrieval_eval::probe_production_scorer_state(&with_index, case, QueryMode::FreshSession),
        ProductionScorerState::QueryAwareWithLexicalIndex,
        "since cas-3b80 project identity alone satisfies has_content(), so even \
         a true cold start stops early-returning onto query-blind Basic at \
         scorer.rs:123"
    );
    assert_eq!(
        retrieval_eval::probe_production_scorer_state(
            &with_index,
            case,
            QueryMode::FreshSessionCarriedPrompt
        ),
        ProductionScorerState::QueryAwareWithLexicalIndex,
        "a cold start in a project with history carries the last prompt forward"
    );

    // A MISSING index is not a hard failure, and this is the part the task
    // brief predicted wrong. `SearchIndex::open` CREATES the directory and an
    // empty index (hybrid_search/search_index_impl.rs:73-96), and
    // `open_with_graph` swallows an entity-store error with `if let Ok(..)`.
    // So the scorer is still constructed, `has_content()` still passes, BM25
    // just returns nothing, and the session lands on Basic + the
    // contextual_overlap_bonus — NOT on the scorer-less fallback.
    let bare = tempfile::tempdir().expect("tempdir");
    let without_index =
        EvalCorpus::materialize(&fixture, bare.path(), TierMode::AllWorking).expect("seed");
    assert!(!without_index.has_search_index());
    assert_eq!(
        retrieval_eval::probe_production_scorer_state(&without_index, case, QueryMode::SeededTask),
        ProductionScorerState::QueryAwareWithoutLexicalIndex,
        "a missing index leaves the scorer constructed and the query live — it \
         must not read as ScorerUnavailable"
    );
    assert_eq!(retrieval_eval::indexed_document_count(without_index.cas_dir()), 0);
    assert!(
        retrieval_eval::indexed_document_count(with_index.cas_dir()) >= fixture.entries.len(),
        "the fixture index must hold at least one document per entry"
    );
}

#[test]
fn a_missing_index_still_ranks_by_the_query_via_the_overlap_bonus() {
    // The consequence of the state above, stated as behaviour rather than as
    // an enum: with NO lexical index at all, production is still
    // query-dependent. Two mechanisms keep it so, and the audit brief missed
    // both: the hybrid search's temporal channel needs no index and returns
    // results anyway (so `score_with_hybrid` is non-empty and the Basic
    // fallback at scorer.rs:165-168 never fires), and contextual_overlap_bonus
    // (scorer.rs:77-116) is added on every branch that gets past :123.
    // "No index => Basic => query-blind" is wrong twice over.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus =
        EvalCorpus::materialize(&fixture, dir.path(), TierMode::AllWorking).expect("seed");
    assert!(!corpus.has_search_index());

    let distinct = distinct_rankings(&fixture, &corpus, QueryMode::SeededTask);
    assert!(
        distinct > 1,
        "with no index the overlap bonus should still vary the ranking; got {distinct}"
    );
}

#[test]
fn the_fast_production_runner_matches_the_real_build_context_path() {
    // The baseline is produced by ProductionRunner, which hoists the store and
    // scorer opens out of the per-case loop (~260ms -> ~30ms per case). That is
    // only legitimate while it stays behaviourally identical to the real
    // `crate::hooks::build_context` call. This is that proof. If someone
    // changes the wiring in cas-cli/src/hooks/context.rs and not in
    // ProductionRunner, this fails — which is the point.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize_with_index(&fixture, dir.path(), TierMode::AllWorking)
        .expect("seed + index");
    let runner = retrieval_eval::ProductionRunner::open(&corpus).expect("runner");

    for query_mode in QueryMode::ALL {
        for case in &fixture.cases {
            let fast = runner.rank(case, query_mode).expect("fast");
            let real =
                retrieval_eval::helpful_memories_production_ranking(&corpus, case, query_mode)
                    .expect("real");
            assert_eq!(
                fast,
                real,
                "{} / {}: the hoisted runner diverged from build_context",
                case.case_id,
                query_mode.as_str()
            );
        }
    }
}

#[test]
fn the_production_selector_is_hermetic_to_home_cas_and_cloud() {
    // Poison the process environment with a logged-in project cloud config
    // whose endpoint is a local trap. The production selector must override
    // both CAS_ROOT and HOME before build_context opens host constraints or
    // loads CloudConfig; otherwise this test observes a request.
    let fixture = fixture();
    let corpus_dir = tempfile::tempdir().expect("corpus tempdir");
    let corpus = EvalCorpus::materialize_with_index(
        &fixture,
        corpus_dir.path(),
        TierMode::AllWorking,
    )
    .expect("seed + index");

    let poison_dir = tempfile::tempdir().expect("poison tempdir");
    let poison_cas = poison_dir.path().join(".cas");
    std::fs::create_dir_all(&poison_cas).expect("poison cas root");
    let listener = TcpListener::bind("127.0.0.1:0").expect("cloud trap");
    listener
        .set_nonblocking(true)
        .expect("nonblocking cloud trap");
    let endpoint = format!("http://{}", listener.local_addr().expect("trap address"));
    std::fs::write(
        poison_cas.join("cloud.json"),
        serde_json::json!({"endpoint": endpoint, "token": "trap-token"}).to_string(),
    )
    .expect("write poison cloud config");

    let _env = TestEnvGuard::temp_home();
    let mut env = _env;
    env.set("HOME", poison_dir.path());
    env.set("CAS_ROOT", &poison_cas);
    env.set("CAS_USER_CLOUD_JSON", poison_cas.join("cloud.json"));

    let (attempt_tx, attempt_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
                    );
                    let _ = attempt_tx.send(true);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let _ = attempt_tx.send(false);
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    let _ = attempt_tx.send(false);
                    return;
                }
            }
        }
    });

    let ranking = retrieval_eval::helpful_memories_production_ranking(
        &corpus,
        &fixture.cases[0],
        QueryMode::SeededTask,
    )
    .expect("hermetic production ranking");
    assert!(!ranking.is_empty(), "the trap must not remove the ranking");
    assert_eq!(
        attempt_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("cloud trap result"),
        false,
        "production ranking attempted a live cloud call"
    );
    server.join().expect("cloud trap thread");
}

#[test]
fn the_production_runner_documents_its_unreplicated_surface() {
    let module = include_str!("../src/retrieval_eval.rs");
    for behavior in [
        "rule/skill/knowledge/agent stores",
        "host-constraints/cloud/mcp-tools sections",
        "hooks.ai_context = false",
        "build_context_ai",
    ] {
        assert!(
            module.contains(behavior),
            "retrieval eval module must disclose unreplicated behavior: {behavior}"
        );
    }
}

#[test]
fn the_fixture_seeds_none_of_the_unreplicated_production_inputs() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("fixture tempdir");
    let corpus = EvalCorpus::materialize(&fixture, dir.path(), TierMode::AllWorking)
        .expect("materialize fixture");
    let connection =
        rusqlite::Connection::open(corpus.cas_dir().join("cas.db")).expect("open fixture db");

    for table in ["rules", "skills", "agents", "knowledge_pages"] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .expect("check fixture table");
        if exists {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                .expect("count fixture table");
            assert_eq!(count, 0, "fixture unexpectedly seeds {table}");
        }
    }

    for path in [
        "cloud.json",
        "proxy_snapshot.json",
        "session_files.json",
        "host_constraints.json",
    ] {
        assert!(
            !corpus.cas_dir().join(path).exists(),
            "fixture unexpectedly seeds omitted production input {path}"
        );
    }
}

#[test]
fn the_full_harness_has_a_named_sixty_second_budget() {
    let module = include_str!("../src/retrieval_eval.rs");
    assert!(
        module.contains("HARNESS_RUNTIME_BUDGET"),
        "the 60s budget must be a named module constant"
    );
    let started = Instant::now();
    let _ = run(&fixture());
    assert!(
        started.elapsed() <= HARNESS_RUNTIME_BUDGET,
        "full retrieval harness exceeded its 60s budget: {:?}",
        started.elapsed()
    );
}

#[test]
fn the_scorer_replica_matches_production_under_the_production_config() {
    // The A/B seam's licence to exist. `ConfigurableHybridScorer` replicates
    // `HybridContextScorer::score_entries` so the channel options production
    // hardcodes can be varied and a ranking question answered by measurement
    // (cas-3b80, and the cas-e7ae fusion decision).
    //
    // Under ScorerConfig::PRODUCTION it must rank identically to the real
    // scorer on every case, in both query modes. If this fails, no number the
    // replica produces means anything.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize_with_index(&fixture, dir.path(), TierMode::AllWorking)
        .expect("seed + index");

    let production = retrieval_eval::ProductionRunner::open(&corpus).expect("production runner");
    let replica =
        retrieval_eval::ProductionRunner::open_with_config(&corpus, ScorerConfig::PRODUCTION)
            .expect("replica runner");

    for query_mode in QueryMode::ALL {
        for case in &fixture.cases {
            assert_eq!(
                production.rank(case, query_mode).expect("production"),
                replica.rank(case, query_mode).expect("replica"),
                "{} / {}: scorer replica diverged from the real HybridContextScorer",
                case.case_id,
                query_mode.as_str()
            );
        }
    }
}

#[test]
fn the_temporal_arms_agree_at_5_today_and_the_harness_says_why() {
    // Exercises the surviving knob, and records an honest asymmetry rather than
    // asserting a flattering one.
    //
    // `enable_temporal` is NOT dead the way the deleted `apply_boosts` flags
    // were. Measured in cas-e979 at the search boundary, flipping it changes
    // `HybridSearch::search`'s result set (189 hits -> 185) and its entire head.
    // That is a real retrieval difference, and it is why the knob was kept.
    //
    // But that difference does NOT survive to the @5 window on this fixture:
    // build_start filters to active-tier entries, `contextual_overlap_bonus`
    // and the high-importance-preference sort dominate the surviving handful,
    // and the top-5 truncates before the temporal reordering matters. So both
    // arms score identically here.
    //
    // Asserting the equality (rather than a difference) is the honest pin: it
    // is what is true today, and a future divergence then arrives explained
    // instead of mysterious. If this starts failing, the temporal channel has
    // begun reaching the metric — re-baseline and record which change did it.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize_with_index(&fixture, dir.path(), TierMode::AllWorking)
        .expect("seed + index");

    let arm = |temporal: bool| {
        retrieval_eval::measure_scorer_arm(
            &fixture,
            &corpus,
            TierMode::AllWorking,
            QueryMode::SeededTask,
            ScorerConfig { temporal },
        )
        .expect("measure")
    };
    let on = arm(true);
    let off = arm(false);
    println!(
        "\n{}",
        retrieval_eval::render_scorer_table(&[on.clone(), off.clone()])
    );

    assert_eq!(
        (on.precision_at_5, on.recall_at_5, on.distinct_rankings),
        (off.precision_at_5, off.recall_at_5, off.distinct_rankings),
        "the temporal channel now reaches the @5 metric. That is a real change, \
         not a broken test — re-baseline and record what caused it."
    );
}

#[test]
fn the_rendered_section_parser_agrees_with_the_production_callback() {
    // The production selector reads its ranking out of the rendered
    // `## Helpful Memories` block, because that is literally what the model
    // receives. That only stays honest if the parser matches the callback the
    // Basic selector uses. Cross-validate the two extraction methods on the
    // same corpus, so parser drift fails here instead of silently reshaping
    // the baseline.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus =
        EvalCorpus::materialize(&fixture, dir.path(), TierMode::AllWorking).expect("seed");

    for case in fixture.cases.iter().take(8) {
        let via_callback = retrieval_eval::helpful_memories_ranking(&corpus, case).expect("rank");
        let via_parser =
            retrieval_eval::helpful_memories_rendered_ranking(&corpus, case).expect("parse");
        assert_eq!(
            via_callback, via_parser,
            "{}: the rendered-section parser disagrees with the on_surfaced callback",
            case.case_id
        );
    }
}

#[test]
fn the_production_selector_does_not_inherit_the_worker_environment() {
    // `build_context_with_stores` renders an Agent Coordination section when
    // CAS_AGENT_ROLE is set, which consumes token budget and can change which
    // memories still fit. The committed baseline must describe a plain
    // operator session, not whichever pane happened to run the harness.
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    let corpus = EvalCorpus::materialize_with_index(&fixture, dir.path(), TierMode::AllWorking)
        .expect("seed + index");
    let case = &fixture.cases[0];

    let baseline_ranking =
        retrieval_eval::helpful_memories_production_ranking(&corpus, case, QueryMode::SeededTask)
            .expect("rank");
    // The env guard must hold on the real build_context path too, not just the
    // hoisted runner — that is the path a future reader will reach for.

    let mut env = TestEnvGuard::new();
    env.set("CAS_AGENT_ROLE", "worker");
    let under_worker_env =
        retrieval_eval::helpful_memories_production_ranking(&corpus, case, QueryMode::SeededTask)
            .expect("rank");

    assert_eq!(
        baseline_ranking, under_worker_env,
        "the production selector leaked CAS_AGENT_ROLE into its measurement"
    );
}

fn distinct_rankings(
    fixture: &EvalFixture,
    corpus: &EvalCorpus,
    query_mode: QueryMode,
) -> usize {
    // Uses the hoisted runner, which
    // `the_fast_production_runner_matches_the_real_build_context_path` proves
    // is ranking-identical to the real build_context call.
    let runner = retrieval_eval::ProductionRunner::open(corpus).expect("runner");
    fixture
        .cases
        .iter()
        .map(|case| runner.rank(case, query_mode).expect("rank"))
        .collect::<HashSet<_>>()
        .len()
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
    let (metrics, _) = run(&fixture);

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
    let (metrics, _details) = run(&fixture);

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
        run(&fixture).0
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
    let (clean, _) = run(&fixture);
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
        let corpus = EvalCorpus::materialize_with_index(&fixture, dir, mode).expect("seed");
        archive_entries(dir, &victims);

        // The production selector is gated too, so the perturbation has to
        // cover it — otherwise `compare` reports missing coverage instead of a
        // regression and the proof is vacuous.
        let runner = retrieval_eval::ProductionRunner::open(&corpus).expect("runner");
        for query_mode in QueryMode::ALL {
            let (m, _) = retrieval_eval::score_in_query_mode(
                SELECTOR_HELPFUL_MEMORIES_PRODUCTION,
                mode,
                Some(query_mode),
                &fixture.cases,
                |c| runner.rank(c, query_mode).unwrap_or_default(),
            );
            damaged.push(m);
        }

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
    let (metrics, _) = run(&fixture);
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
        query_mode: "n/a".to_string(),
        distinct_rankings: 1,
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
