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
        "target=non-Rust (vitest a) --jobs 16 loops=3 result=PASS",
        "target=vitest src/example.test.ts parallelism=16 loops=3 result=PASS",
        "target=jest tests/example.test.js concurrency=16 loops=4 exit=0",
        "target=custom-runner a --jobs=16 3x; SUCCESS",
        "target=whole target cargo test -j16; 3 loops; PASS",
        "whole target under -j16, 3 loops; result: PASS",
        "non-Rust target: pnpm test -j16; 3 runs; PASS",
        "entire suite; -j 16; runs 1 2 3; status=0",
    ] {
        let task = concurrency_task(receipt);
        validate_risk_close_proofs(&task, &[], std::path::Path::new("."))
            .unwrap_or_else(|error| panic!("valid receipt {receipt}: {error}"));
        assert!(declared_risk_close_gaps(&task, &[]).is_empty());
    }
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
            "target=vitest parallelism=16 loops=2 result=PASS",
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
        let gaps = declared_risk_close_gaps(&task, &[]).join("; ");
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
    assert!(declared_risk_close_gaps(&persisted, &[]).is_empty());
}
