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
