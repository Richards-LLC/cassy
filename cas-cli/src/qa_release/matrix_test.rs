// Included in qa_release::tests: the golden-set matrix (finding 18).
/// A golden set expands into cells × runs rows: preflight needs consumes ×
/// runs × cells, a duplicate independent run is rejected and stays owed, a
/// band miss is FAIL with its %, a labeled cell with no measurement is owed,
/// so is one whose label is not a number, an unlabeled cell keeps its runner
/// verdict as debt, and the pack counts cells × runs. A duplicate journey id,
/// a row-id clash or an expansion over qa.release.max_cells refuses at start
/// and persists nothing.
#[test]
fn matrix() {
    use std::sync::atomic::AtomicBool;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let gate = run_support::gate(
        &mut env,
        &[
            ("env", "FAKE_SERVED_SHA,FAKE_BALANCE,FAKE_CALL_LOG,FAKE_RUN_DIR"),
            ("suites", r#"{"golden":{"journeys":["golden"],"wall_clock_mins":5,"concurrency":2}}"#),
        ],
    );
    env.set("FAKE_BALANCE", r#"{"credits":20}"#);
    let root = gate.project.root();
    std::fs::write(
        root.join("docs/qa/golden.tsv"),
        format!(
            "id\tsha256\tstorage_key\tlabels\nA\t{0}\tgolden/a.mp4\t{{\"INV-COUNT\": 100}}\nB\t{0}\tgolden/b.mp4\t{{\"INV-COUNT\": 50}}\nC\t{0}\tgolden/c.mp4\t\nD\t{0}\tgolden/d.mp4\t{{\"INV-COUNT\": \"high\"}}\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
    let registry = |extra: &str| {
        format!(
            "version: 1
resources:
  credits: {{kind: balance}}
invariants:
  INV-COUNT: {{statement: Slide count, check: Count slides, labeled: true, definition: Authored slides, compare: {{banded: {{min_ratio: 0.9}}}}}}
journeys:
  - id: golden
    status: live
    owners: [apps/web/**]
    spec: e2e/golden.spec.ts
    release_always: true
    consumes: {{credits: 1}}
    invariants: [INV-COUNT]
    fixture_set: docs/qa/golden.tsv
    matrix: {{route: [web, app]}}
    runs: 2
    independence_key: run.id
{extra}"
        )
    };
    let set = |key: &str, value: &str| {
        let mut config = crate::config::Config::load(&gate.cas).unwrap();
        config.set(key, value).unwrap();
        config.save(&gate.cas).unwrap();
    };

    let clash = gate.project.commit(
        "docs/qa/journeys.yaml",
        &registry("  - id: golden.A.web.r1\n    status: live\n    spec: e2e/other.spec.ts\n    release_always: true\n"),
    );
    let refusal = run::start(&gate.cas, &clash).unwrap_err().to_string();
    assert!(refusal.contains("row id golden.A.web.r1 is planned by journeys golden and golden.A.web.r1"), "{refusal}");
    assert!(run::load(&gate.cas, &clash).unwrap().is_none(), "a refused start persists nothing");
    let twice = gate.project.commit(
        "docs/qa/journeys.yaml",
        &registry("  - id: golden\n    status: live\n    spec: e2e/other.spec.ts\n    release_always: true\n"),
    );
    let refusal = run::start(&gate.cas, &twice).unwrap_err().to_string();
    assert!(refusal.contains("journey id golden is declared twice"), "{refusal}");

    let sha = gate.project.commit("docs/qa/journeys.yaml", &registry(""));
    set("qa.release.max_cells", "15");
    let refusal = run::start(&gate.cas, &sha).unwrap_err().to_string();
    assert!(refusal.contains("golden expands to 8 cells × 2 runs = 16 rows, over qa.release.max_cells 15"), "{refusal}");
    assert!(run::load(&gate.cas, &sha).unwrap().is_none(), "a refused start persists nothing");
    set("qa.release.max_cells", "500");

    let runs = tempfile::tempdir().unwrap();
    env.set("FAKE_RUN_DIR", runs.path());
    for record in ["A", "B", "C", "D"] {
        for route in ["web", "app"] {
            for run in ["r1", "r2"] {
                let row = format!("{record}.{route}.{run}");
                let id = if row == "A.web.r2" { "A.web.r1".to_string() } else { row.clone() };
                let measured = match (record, row.as_str()) {
                    (_, "B.app.r1") => r#","measured":{"INV-COUNT":40}"#,
                    (_, "A.app.r2") | ("C", _) => "",
                    ("A", _) | ("D", _) => r#","measured":{"INV-COUNT":100}"#,
                    _ => r#","measured":{"INV-COUNT":50}"#,
                };
                std::fs::write(
                    runs.path().join(format!("golden.{row}.json")),
                    format!(r#""subjects":{{"run.id":"{id}"}}{measured}"#),
                )
                .unwrap();
            }
        }
    }
    let started = run::start(&gate.cas, &sha).unwrap();
    assert_eq!(started.plan.len(), 16);
    assert_eq!(started.resources.need["credits"], 16, "consumes × runs × cells");
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();

    let pack = pack::Pack::load(&pack::pack_dir(&gate.cas, &sha).unwrap()).unwrap();
    let word = |id: &str| pack.rows.iter().find(|r| r.id == id).unwrap().verdict.to_string();
    assert!(word("golden.A.web.r2").contains("duplicate run.id=A.web.r1 (2 rows)"), "{}", word("golden.A.web.r2"));
    assert_eq!(word("golden.B.app.r1"), "FAIL: band miss: INV-COUNT measured 40 is 80.0% of baseline 50");
    assert_eq!(word("golden.A.app.r2"), "NOT_EXERCISED{automation}: no measured value for labeled INV-COUNT");
    assert_eq!((word("golden.A.app.r1"), word("golden.C.web.r1")), ("PASS".into(), "PASS".into()));
    assert_eq!(word("golden.D.web.r1"), "NOT_EXERCISED{automation}: label not gradable (not a number) for INV-COUNT");
    let cell = pack.rows.iter().find(|r| r.id == "golden.B.app.r1").unwrap();
    assert_eq!((cell.cell.as_deref(), cell.run), (Some("fixture=B route=app"), Some(1)));
    assert_eq!(
        pack.matrix,
        [matrix::Coverage {
            journey: "golden".into(),
            cells: 8,
            runs: 2,
            required: 16,
            counted: 10,
            unlabeled: vec!["C.web".into(), "C.app".into()],
            missing: ["A.web.r2", "A.app.r2", "D.web.r1", "D.web.r2", "D.app.r1", "D.app.r2"]
                .map(|row| format!("golden.{row}"))
                .to_vec(),
        }]
    );
    assert_eq!(pack.gate.verdict, verdict::GateVerdict::Failed);
    assert!(
        comment::render_comment(&pack)
            .contains("`golden`: 10/16 runs (8 cells × 2 runs); owed: 6; unlabeled cells (debt): C.web, C.app")
    );
}
