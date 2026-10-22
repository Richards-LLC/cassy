//! GH #1222: exercise the production risk close gate with typed task notes.
use super::*;

const ISSUE_RECEIPT: &str = "LOADED_PROOF: target=explicit non-Rust (vitest <file> + jest <file>) sha=d910dbfce parallelism=-j16 (--jobs 16: 16 concurrent test processes per loop) loops=3 result=PASS (48/48 runs passed, 0 failed) log=<artifacts>/cas-60ab/loaded16/summary.txt";

fn concurrency_task(notes: &str) -> Task {
    let mut task = Task::new(
        "cas-a576-fixture".into(),
        "Non-Rust concurrency proof".into(),
    );
    task.risk = vec![TaskRisk::Concurrency];
    // This is the actual note_type=loaded_proof rendering around the body.
    task.notes = format!("[2026-10-15 22:25] 🧪 LOADED_PROOF {notes}");
    task
}

#[test]
fn cas_a576_exact_issue_receipt_passes_real_risk_close_gate() {
    let task = concurrency_task(ISSUE_RECEIPT);
    validate_risk_close_proofs(&task, &[], std::path::Path::new("."))
        .expect("the exact GH #1222 typed receipt must satisfy the real close gate");
}

#[test]
fn cas_a576_rust_module_filter_target_is_refused() {
    for target in [
        "cas::some::single_test",
        "cargo test some::single_test",
        "whole::single_test",
        "vitest_lookalike",
        "jest::single_test",
        "go build",
        "custom-runner a",
        "explicit",
        "explicit Rust (cas::some::single_test)",
    ] {
        let task = concurrency_task(&format!(
            "target={target}; parallelism=16; loops=3; result=PASS"
        ));
        let error = validate_risk_close_proofs(&task, &[], std::path::Path::new("."))
            .expect_err("a structured Rust subset must not satisfy whole-target scope");
        assert!(error.contains("missing=[target]"), "{target}: {error}");
    }
}

#[test]
fn cas_a576_refusal_reports_parsed_missing_fields_and_example() {
    let task =
        concurrency_task("non-Rust target: vitest src/example.test.ts --jobs 16; result=PASS");
    let error = validate_risk_close_proofs(&task, &[], std::path::Path::new(".")).unwrap_err();
    for expected in [
        "parsed=[target, parallelism, result]",
        "missing=[loops]",
        "LOADED_PROOF: target=",
        "loops=3",
        "result=PASS",
    ] {
        assert!(
            error.contains(expected),
            "missing diagnostic {expected}: {error}"
        );
    }
}

#[test]
fn cas_a576_split_notes_report_each_incomplete_receipt() {
    let mut task = concurrency_task("whole target; -j16");
    task.notes
        .push_str("\n[2026-10-15 22:26] 🧪 LOADED_PROOF 3 loops; result=PASS");
    let error = validate_risk_close_proofs(&task, &[], std::path::Path::new(".")).unwrap_err();
    for expected in [
        "note 1",
        "parsed=[target, parallelism]",
        "missing=[loops, result]",
        "note 2",
        "parsed=[loops, result]",
        "missing=[target, parallelism]",
    ] {
        assert!(
            error.contains(expected),
            "missing per-note diagnostic {expected}: {error}"
        );
    }
}

#[test]
fn structured_targets_accept_non_rust_runners_and_parallelism_forms_without_sha() {
    for receipt in [
        "target=explicit non-Rust (vitest a + jest b) parallelism=-j16 loops=3 result=PASS",
        "target=explicit non-Rust (vitest a) --jobs 16 loops=3 result=PASS",
        "target=vitest src/example.test.ts parallelism=16 loops=3 result=PASS",
        "target=jest tests/example.test.js concurrency=16 loops=4 exit=0",
        "target=explicit non-Rust (custom-runner a) --jobs=16 3x; SUCCESS",
        "target=pytest tests/test_example.py parallelism=16 loops=3 result=PASS",
        "target=go test ./... parallelism=16 loops=3 result=PASS",
        "target=whole parallelism=16 loops=3 result=PASS",
        "target=entire parallelism=16 loops=3 result=PASS",
        "target=full parallelism=16 loops=3 result=PASS",
        "target=whole target cargo test -j16; 3 loops; PASS",
        "whole target under -j16, 3 loops; result: PASS",
        "non-Rust target: pnpm test -j16; 3 runs; PASS",
        "entire suite; -j 16; runs 1 2 3; status=0",
    ] {
        let task = concurrency_task(receipt);
        validate_risk_close_proofs(&task, &[], std::path::Path::new("."))
            .unwrap_or_else(|error| panic!("valid receipt {receipt}: {error}"));
        assert!(declared_risk_close_gaps(&task, &[], std::path::Path::new(".")).is_empty());
    }
}

// GH #1222 quotes one full receipt and the shorthand `-j16 x3`. The other
// rows isolate its documented forms; they are derived fixtures, not quotes.
const ISSUE_FORMS: &[(&str, &str)] = &[
    ("GH #1222 verbatim full line", ISSUE_RECEIPT),
    (
        "GH #1222 shorthand in whole-target receipt",
        "whole target; -j16 x3; PASS",
    ),
    (
        "derived jobs option",
        "target=explicit non-Rust (vitest <file> + jest <file>) --jobs 16 loops=3 result=PASS",
    ),
    (
        "derived parallelism option",
        "target=explicit non-Rust (vitest <file> + jest <file>) parallelism=-j16 loops=3 result=PASS",
    ),
];

async fn exercise_close_receipts_cas_69b4(receipts: &[(&str, &str)], missing: Option<&str>) {
    use crate::mcp::CasService;
    use crate::store::{
        open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
    };
    use crate::test_support::TestEnvGuard;
    use cas_types::{Agent, AgentRole};
    use rmcp::handler::server::wrapper::Parameters;

    let mut env = TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    env.set_current_dir(dir.path());
    std::fs::write(
        root.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .unwrap();
    open_store(&root).unwrap().init().unwrap();
    open_rule_store(&root).unwrap().init().unwrap();
    open_skill_store(&root).unwrap().init().unwrap();
    let store = open_task_store(&root).unwrap();
    store.init().unwrap();
    let agents = open_agent_store(&root).unwrap();
    agents.init().unwrap();
    let caller = Agent::new_with_role(
        "receipt-caller".into(),
        "receipt-caller".into(),
        AgentRole::Standard,
    );
    agents.register(&caller).unwrap();
    let core = CasCore::with_daemon(root, None, None);
    core.set_agent_id_for_testing(caller.id);
    let service = CasService::new(core, None);

    for (index, (name, receipt)) in receipts.iter().enumerate() {
        let mut task = concurrency_task("");
        task.id = format!("cas-69b4-{index}");
        task.status = TaskStatus::InProgress;
        task.notes.clear();
        store.add(&task).unwrap();
        let request = serde_json::from_value(serde_json::json!({
            "action": "notes", "id": task.id, "notes": receipt, "note_type": "loaded_proof"
        }))
        .unwrap();
        let note = service.task(Parameters(request)).await.unwrap();
        assert_ne!(note.is_error, Some(true), "{name}: {note:?}");
        let request = serde_json::from_value(serde_json::json!({
            "action": "close", "id": task.id, "reason": "Validated receipt fixture"
        }))
        .unwrap();
        let response = service.task(Parameters(request)).await.unwrap();
        let text = response
            .content
            .iter()
            .filter_map(|content| match &content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let persisted = store.get(&task.id).unwrap();
        if missing.is_none() {
            assert_ne!(response.is_error, Some(true), "{name}: {receipt}: {text}");
            assert_eq!(persisted.status, TaskStatus::Closed, "{name}: {text}");
            assert!(
                declared_risk_close_gaps(&persisted, &[], dir.path()).is_empty(),
                "{name}: audit differs"
            );
        } else {
            assert_eq!(
                response.is_error,
                Some(true),
                "{name}: accepted {receipt}: {text}"
            );
            assert_eq!(persisted.status, TaskStatus::InProgress, "{name}: {text}");
            let missing = format!("missing=[{}]", missing.unwrap());
            assert!(
                text.contains("loaded_proof") && text.contains(&missing),
                "{name}: {text}"
            );
            assert!(text.contains("parsed=[target,"), "{name}: {text}");
            assert!(text.contains("Accepted example:"), "{name}: {text}");
            assert!(
                declared_risk_close_gaps(&persisted, &[], dir.path())
                    .join(";")
                    .contains(&missing),
                "{name}: audit differs"
            );
        }
    }
}

#[tokio::test]
async fn cas_69b4_close_handler_refuses_borrowed_loop_counts() {
    exercise_close_receipts_cas_69b4(
        &[
            (
                "parallelism before loops",
                "whole target parallelism=16 loops=2 result=PASS",
            ),
            (
                "loops before parallelism",
                "whole target loops=2 parallelism=16 result=PASS",
            ),
            (
                "jobs before loop",
                "whole target jobs=16 loop=2 result=PASS",
            ),
            (
                "loop before jobs",
                "whole target loop=2 jobs=16 result=PASS",
            ),
            (
                "structured jobs",
                "target=vitest jobs=16 loops=2 result=PASS",
            ),
            (
                "colon fields",
                "whole target concurrency: 16 runs: 2 result=PASS",
            ),
            (
                "prose label order",
                "whole target parallelism 16 loops 2 result=PASS",
            ),
            (
                "other assigned field",
                "whole target -j16 receipt=48 loops=2 result=PASS",
            ),
        ],
        Some("loops"),
    )
    .await;
}

#[tokio::test]
async fn cas_69b4_close_handler_accepts_issue_receipt_forms() {
    exercise_close_receipts_cas_69b4(ISSUE_FORMS, None).await;
}

#[test]
fn missing_or_contradictory_fields_remain_refused_with_precise_diagnostics() {
    for (receipt, expected) in [
        ("-j16 loops=3 result=PASS", "missing=[target]"),
        ("target= -j16 loops=3 result=PASS", "missing=[target]"),
        ("target=loops=3 -j16 result=PASS", "missing=[target]"),
        (
            "log=target=vitest -j16 loops=3 result=PASS",
            "missing=[target]",
        ),
        ("target=vitest loops=3 result=PASS", "missing=[parallelism]"),
        (
            "target=vitest -j160 loops=3 result=PASS",
            "missing=[parallelism]",
        ),
        (
            "target=vitest concurrency=160 loops=3 result=PASS",
            "missing=[parallelism]",
        ),
        (
            "target=vitest parallelism=16; loops=2 result=PASS",
            "missing=[loops]",
        ),
        (
            "target=vitest parallelism=16 receipt=1234 result=PASS",
            "missing=[loops]",
        ),
        ("target=vitest parallelism=16 loops=3", "missing=[result]"),
        (
            "target=vitest parallelism=16 loops=3 result=FAIL",
            "missing=[result]",
        ),
        (
            "target=vitest parallelism=16 loops=3 result=PASS exit=1",
            "missing=[result]",
        ),
        (
            "target=vitest parallelism=16 loops=3 result=PASS status=2",
            "missing=[result]",
        ),
    ] {
        let task = concurrency_task(receipt);
        let error =
            validate_risk_close_proofs(&task, &[], std::path::Path::new(".")).expect_err(receipt);
        assert!(error.contains(expected), "{receipt}: {error}");
        assert!(error.contains("parsed=["), "{error}");
        assert!(
            error.contains("Accepted example: LOADED_PROOF: target="),
            "{error}"
        );
        let gaps = declared_risk_close_gaps(&task, &[], std::path::Path::new(".")).join("; ");
        assert!(gaps.contains(expected), "audit differs: {gaps}");
        assert!(
            gaps.contains("Accepted example: LOADED_PROOF: target="),
            "{gaps}"
        );
    }
}

#[test]
fn no_typed_note_reports_all_missing_fields_and_executable_example() {
    let mut task = concurrency_task("");
    task.notes = "progress: target=vitest parallelism=16 loops=3 result=PASS".into();
    let error = validate_risk_close_proofs(&task, &[], std::path::Path::new("."))
        .expect_err("untyped progress cannot satisfy concurrency risk");
    assert!(error.contains("typed note_type=loaded_proof"), "{error}");
    assert!(
        error.contains("parsed=[] missing=[target, parallelism, loops, result]"),
        "{error}"
    );
    let example = error
        .split("Accepted example: ")
        .nth(1)
        .unwrap()
        .split(". Add one with")
        .next()
        .unwrap();
    validate_risk_close_proofs(&concurrency_task(example), &[], std::path::Path::new("."))
        .expect("the refusal's accepted example must pass without adding SHA/log");
}

#[tokio::test]
async fn mcp_typed_note_persistence_preserves_issue_receipt_for_close_and_audit() {
    use crate::mcp::tools::TaskNotesRequest;
    use crate::store::{open_agent_store, open_task_store};
    use crate::test_support::TestEnvGuard;
    use cas_types::{Agent, AgentRole};
    use rmcp::handler::server::wrapper::Parameters;

    let mut env = TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    env.set("CAS_ROOT", &root);
    let store = open_task_store(&root).unwrap();
    store.init().unwrap();
    let agents = open_agent_store(&root).unwrap();
    agents.init().unwrap();
    let caller = Agent::new_with_role(
        "receipt-caller".into(),
        "receipt-caller".into(),
        AgentRole::Standard,
    );
    agents.register(&caller).unwrap();
    let core = CasCore::with_daemon(root, None, None);
    core.set_agent_id_for_testing(caller.id);
    let mut task = concurrency_task("");
    task.notes.clear();
    store.add(&task).unwrap();

    core.cas_task_notes(Parameters(TaskNotesRequest {
        id: task.id.clone(),
        note: ISSUE_RECEIPT.into(),
        note_type: "loaded_proof".into(),
        supervisor_override: None,
        reason: None,
    }))
    .await
    .expect("the real MCP note writer must persist the receipt");

    let persisted = store.get(&task.id).unwrap();
    assert!(
        persisted
            .notes
            .contains("🧪 LOADED_PROOF LOADED_PROOF: target=explicit non-Rust")
    );
    validate_risk_close_proofs(&persisted, &[], dir.path())
        .expect("real typed-note write/reload must satisfy the production close gate");
    assert!(declared_risk_close_gaps(&persisted, &[], dir.path()).is_empty());
}

// Fixture families accepted before cas-69b4. Keep this inventory explicit so
// narrowing numeric ownership cannot silently drop valid prose/field forms.
const LEGACY_FORMS: &[(&str, &str)] = &[
    (
        "numeric fields",
        "whole target parallelism=16 loops=3 result=PASS",
    ),
    ("reverse prose fields", "whole target 16 jobs 3 loops; PASS"),
    ("forward prose fields", "whole target jobs 16 loops 3; PASS"),
    ("loop prose first", "whole target 3 loops 16 jobs; PASS"),
    (
        "reverse parallelism",
        "whole target 16 parallelism; 3 loops; PASS",
    ),
    (
        "reverse concurrency",
        "whole target 16 concurrency; 3 loops; PASS",
    ),
    ("jobs colon", "whole target jobs: 16 loops: 3 result=PASS"),
    (
        "concurrency colon",
        "whole target concurrency:16 runs:4 result=PASS",
    ),
    (
        "parallelism colon",
        "whole target parallelism:16 runs:4 result=PASS",
    ),
    ("option compact", "whole target -j16; 3 loops; PASS"),
    ("option spaced", "entire suite; -j 16; runs 1 2 3; status=0"),
    ("long option spaced", "full suite --jobs 16; 3x; SUCCESS"),
    (
        "long option equals",
        "non-Rust target: pnpm test --jobs=16; three runs; status 0",
    ),
    (
        "greater run count",
        "whole target cargo test -j16; loops: 32; result=SUCCESS",
    ),
    ("punctuated fields", "full target | 16 jobs, 3 runs; PASS"),
    (
        "structured full",
        "target=full parallelism=16 loops=3 result=PASS",
    ),
    (
        "structured entire",
        "target=entire parallelism=16 loops=3 result=PASS",
    ),
    (
        "structured whole",
        "target=whole target cargo test -j16; 3 loops; PASS",
    ),
    (
        "structured vitest",
        "target=vitest src/example.test.ts parallelism=16 loops=3 result=PASS",
    ),
    (
        "structured jest",
        "target=jest tests/example.test.js concurrency=16 loops=4 exit=0",
    ),
    (
        "structured pytest",
        "target=pytest tests/test_example.py parallelism=16 loops=3 result=PASS",
    ),
    (
        "structured go",
        "target=go test ./... parallelism=16 loops=3 result=PASS",
    ),
    (
        "explicit custom runner",
        "target=explicit non-Rust (custom-runner a) --jobs=16 3x; SUCCESS",
    ),
];

#[tokio::test]
async fn cas_69b4_close_handler_preserves_legacy_receipt_families() {
    exercise_close_receipts_cas_69b4(LEGACY_FORMS, None).await;
}

#[tokio::test]
async fn cas_69b4_close_handler_refuses_borrowed_parallelism() {
    exercise_close_receipts_cas_69b4(
        &[
            (
                "loops before jobs",
                "whole target loops=16 jobs=2 result=PASS",
            ),
            (
                "jobs before loops",
                "whole target jobs=2 loops=16 result=PASS",
            ),
            (
                "loops before parallelism",
                "whole target loops=16 parallelism=2 result=PASS",
            ),
            (
                "loops before concurrency",
                "whole target loops=16 concurrency: 2 result=PASS",
            ),
            (
                "unknown assigned count",
                "whole target receipt=16 jobs 2; 3 loops; PASS",
            ),
        ],
        Some("parallelism"),
    )
    .await;
}

#[test]
fn cas_69b4_numeric_and_word_run_aliases_keep_their_own_values() {
    for label in [
        "loop",
        "loops",
        "run",
        "runs",
        "iteration",
        "iterations",
        "time",
        "times",
    ] {
        for runs in [
            format!("{label}=3"),
            format!("{label}: 4"),
            format!("{label} 3"),
            format!("3 {label}"),
            format!("three {label}"),
        ] {
            let receipt = format!("whole target; -j16; {runs}; PASS");
            validate_risk_close_proofs(&concurrency_task(&receipt), &[], std::path::Path::new("."))
                .expect(&receipt);
        }
        for runs in [
            format!("{label}=2"),
            format!("{label}: 2"),
            format!("{label} 2"),
        ] {
            let receipt = format!("whole target jobs=16 {runs} result=PASS");
            let error = validate_risk_close_proofs(
                &concurrency_task(&receipt),
                &[],
                std::path::Path::new("."),
            )
            .expect_err(&receipt);
            assert!(error.contains("missing=[loops]"), "{receipt}: {error}");
        }
    }
    for runs in [
        "x0",
        "x2",
        "2x",
        "x3lookalike",
        "lookalikex3",
        "x4294967296",
        "loops=4294967296",
    ] {
        let receipt = format!("whole target; -j16; {runs}; PASS");
        let error =
            validate_risk_close_proofs(&concurrency_task(&receipt), &[], std::path::Path::new("."))
                .expect_err(&receipt);
        assert!(error.contains("missing=[loops]"), "{receipt}: {error}");
        assert!(
            error.contains("3x or -j16 x3"),
            "accepted forms missing: {error}"
        );
    }
}

#[tokio::test]
async fn cas_69b4_review_x86_architecture_is_not_repetition() {
    exercise_close_receipts_cas_69b4(
        &[
            (
                "x86 architecture after explicit low count",
                "target=whole parallelism=16 loops=1 result=PASS host x86_64",
            ),
            (
                "x86 architecture after jobs",
                "whole target -j16 x86_64; PASS",
            ),
            (
                "x86 standalone host",
                "whole target -j16; host x86_64; PASS",
            ),
        ],
        Some("loops"),
    )
    .await;
}

#[tokio::test]
async fn cas_69b4_review_x64_architecture_is_not_repetition() {
    exercise_close_receipts_cas_69b4(
        &[
            ("x64 standalone host", "whole target -j16; host x64; PASS"),
            (
                "x64 after explicit low count",
                "target=whole parallelism=16 loops=1 result=PASS host x64",
            ),
        ],
        Some("loops"),
    )
    .await;
}

#[tokio::test]
async fn cas_69b4_review_explicit_low_counts_win_over_shorthand() {
    exercise_close_receipts_cas_69b4(
        &[
            (
                "loops2 after bound x3",
                "whole target -j16 x3 loops=2 result=PASS",
            ),
            (
                "loops2 before bound x3",
                "whole target loops=2 -j16 x3 result=PASS",
            ),
            (
                "runs2 after bound x3",
                "whole target --jobs 16 x3 runs=2 result=PASS",
            ),
            (
                "runs2 colon before 3x",
                "whole target runs: 2 -j16 3x; PASS",
            ),
            ("loops2 before 3x", "whole target loops=2 -j16 3x; PASS"),
            (
                "loops2 after 3x",
                "whole target -j16 3x loops=2 result=PASS",
            ),
            (
                "zero explicit runs",
                "whole target --jobs 16 x3 runs=0 result=PASS",
            ),
        ],
        Some("loops"),
    )
    .await;
}

#[tokio::test]
async fn cas_69b4_review_x_repetition_requires_adjacent_parallelism() {
    exercise_close_receipts_cas_69b4(
        &[
            (
                "x3 unbound after result",
                "whole target parallelism=16 result=PASS x3",
            ),
            ("x3 across delimiter", "whole target -j16; x3; PASS"),
            (
                "x3 after unrelated number",
                "whole target -j16 host=16 x3; PASS",
            ),
            ("x3 underscore suffix", "whole target -j16 x3_host; PASS"),
        ],
        Some("loops"),
    )
    .await;
}

#[tokio::test]
async fn cas_69b4_review_bound_repetition_forms_remain_accepted() {
    exercise_close_receipts_cas_69b4(
        &[
            ("compact j repetition", "whole target -j16 x3; PASS"),
            ("spaced j repetition", "whole target -j 16 x4; PASS"),
            ("jobs option repetition", "whole target --jobs 16 x3; PASS"),
            ("jobs equals repetition", "whole target --jobs=16 x3; PASS"),
            (
                "parallelism repetition",
                "whole target parallelism=16 x3; PASS",
            ),
            (
                "parallelism flag repetition",
                "whole target parallelism=-j16 x3; PASS",
            ),
            (
                "concurrency repetition",
                "whole target concurrency: 16 x3; PASS",
            ),
            (
                "valid explicit count with host architecture",
                "whole target -j16 loops=3 host=x86_64 result=PASS",
            ),
            (
                "valid explicit count with host x64",
                "whole target -j16 runs=3 host x64 result=PASS",
            ),
        ],
        None,
    )
    .await;
}
