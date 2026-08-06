//! End-to-end integration tests for the cas-code-review pipeline (cas-22fa).
//!
//! These tests exercise the full deterministic Rust core of EPIC cas-0750
//! Subsystem A as a single composed pipeline:
//!
//!   `Vec<ReviewerOutput>`
//!     → [`merge_findings`]                       (Unit 5)
//!     → [`autofix_loop`]                         (Unit 7)
//!     → [`route_residual_to_tasks`]              (Unit 8)
//!     → assemble [`ReviewOutcome`]               (gate envelope)
//!     → serialize → JSON
//!     → deserialize → [`ReviewOutcome::validate`]
//!     → [`evaluate_gate`]                        (Unit 9 policy)
//!     → final close decision
//!
//! The orchestrator (Unit 4) is intentionally *not* in this composition:
//! it is an LLM-driven prompt skill that produces the `Vec<ReviewerOutput>`
//! we feed in here. The personas (Unit 2) are also LLM prompts. The
//! deterministic pipeline that gates `task.close` is everything that
//! happens *after* the LLM returns its JSON, and that is what these tests
//! cover end-to-end.
//!
//! The six scenarios from cas-22fa's test plan are each represented:
//!
//! 1. Clean diff → close succeeds, zero residual
//! 2. safe_auto P3 → autofix applies it, zero residual, close succeeds
//! 3. manual P1 → residual carries the finding, gate allows close, a
//!    follow-up task is created via review-to-task with severity:P1
//! 4. P0 → residual carries the finding, gate hard-blocks the close,
//!    block message names the file:line and proposes the supervisor
//!    override path
//! 5. Multi-persona overlap → merge dedupes + boosts confidence + the
//!    pre-existing partition keeps base-ref debt out of the gate
//! 6. Pre-existing P0 → defense in depth at the gate level: even if a
//!    pre-existing P0 sneaks into the residual, the gate must not block
//!
//! Together they validate the EPIC's primary close-flow promise: workers
//! see the right behavior end-to-end, not just at the unit boundary.

use std::cell::RefCell;
use std::rc::Rc;

use cas_store::code_review::{
    autofix_loop,
    close_gate::{evaluate_gate, format_block_message, GateDecision},
    merge_findings, route_residual_to_tasks, AutofixOutcome, ExitReason, FixerResult, RouteOutcome,
};
use cas_types::{AutofixClass, Finding, FindingSeverity, Owner, ReviewOutcome, ReviewerOutput};

// --------------------------------------------------------------------------
// Fixture helpers
// --------------------------------------------------------------------------

fn finding(
    title: &str,
    sev: FindingSeverity,
    file: &str,
    line: u32,
    conf: f32,
    owner: Owner,
    class: AutofixClass,
    pre_existing: bool,
) -> Finding {
    Finding {
        title: title.to_string(),
        severity: sev,
        file: file.to_string(),
        line,
        why_it_matters: format!("breaks something at {file}:{line}"),
        autofix_class: class,
        owner,
        confidence: conf,
        evidence: vec![format!("// {file}:{line}\nlet x = something();")],
        pre_existing,
        suggested_fix: None,
        requires_verification: false,
    }
}

fn envelope(reviewer: &str, findings: Vec<Finding>) -> ReviewerOutput {
    ReviewerOutput {
        reviewer: reviewer.to_string(),
        findings,
        residual_risks: vec![],
        testing_gaps: vec![],
        skipped_reason: None,
    }
}

/// Run the deterministic post-LLM pipeline end-to-end. Returns the
/// pieces a real call site (`close_ops::run_code_review_gate`) needs
/// to make a final close decision.
struct PipelineResult {
    autofix: AutofixOutcome,
    route: RouteOutcome,
    envelope: ReviewOutcome,
    /// Round-trip JSON of `envelope`. Validates that the worker → close
    /// boundary serialization shape is intact at integration scale.
    envelope_json: String,
    /// Final gate decision after deserializing `envelope_json` and
    /// running `evaluate_gate` on the residual.
    decision: GateDecision,
}

fn run_pipeline(envelopes: Vec<ReviewerOutput>) -> PipelineResult {
    // Stage A: merge.
    let merged = merge_findings(envelopes).expect("merge succeeds on valid envelopes");

    // Stage B: autofix loop. The fixer stub "applies" every safe_auto
    // finding it sees in one round, and the rereviewer returns a
    // MergedFindings with the safe_autos stripped. This mirrors the
    // happy-path behavior of the real LLM fixer for tests that
    // exercise the loop without actually invoking a model.
    let merged_for_loop = merged.clone();
    let safe_auto_titles: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let titles_for_fixer = safe_auto_titles.clone();
    let titles_for_rereview = safe_auto_titles.clone();
    let stripped_after_round = Rc::new(RefCell::new(merged_for_loop.clone()));
    let stripped_handle = stripped_after_round.clone();

    let autofix = autofix_loop(
        merged_for_loop,
        |safe_auto| {
            // Pretend we applied every safe_auto in the queue.
            titles_for_fixer
                .borrow_mut()
                .extend(safe_auto.iter().map(|f| f.title.clone()));
            FixerResult {
                applied: safe_auto.to_vec(),
                skipped: vec![],
                crashed: false,
            }
        },
        || {
            // After a productive round, hand back a merged state with
            // the safe_auto findings removed (the rereviewer would see
            // them gone from the patched tree).
            let to_remove = titles_for_rereview.borrow().clone();
            let mut next = stripped_handle.borrow().clone();
            next.pr_introduced.retain(|f| !to_remove.contains(&f.title));
            *stripped_handle.borrow_mut() = next.clone();
            Ok(next)
        },
    );

    // Stage C: review-to-task. Closure-injected persistence stubs that
    // record what would have been written.
    let next_id = Rc::new(RefCell::new(1u32));
    let id_for_create = next_id.clone();
    let route = route_residual_to_tasks(
        &autofix.residual,
        |_external_ref| None, // no pre-existing tasks
        |draft| {
            let mut n = id_for_create.borrow_mut();
            let id = format!("task-{:04}", *n);
            *n += 1;
            // Touch a draft field to keep clippy happy and assert the
            // closure receives the real draft.
            assert!(!draft.title.is_empty());
            Ok(id)
        },
        |_id, _draft| Ok(()),
    )
    .expect("route succeeds on well-formed residual");

    // Stage D: assemble the gate envelope, round-trip through JSON to
    // validate the worker → close boundary, then run the gate policy.
    let envelope = ReviewOutcome {
        residual: autofix.residual.clone(),
        pre_existing: autofix.pre_existing.clone(),
        mode: "autofix".to_string(),
        execution: None,
    };
    let envelope_json = serde_json::to_string(&envelope).expect("envelope serializes");
    let parsed: ReviewOutcome = serde_json::from_str(&envelope_json).expect("envelope round-trips");
    parsed.validate().expect("envelope validates");

    let decision = evaluate_gate(&parsed.residual);

    PipelineResult {
        autofix,
        route,
        envelope,
        envelope_json,
        decision,
    }
}

// --------------------------------------------------------------------------
// Scenario 1: clean diff
// --------------------------------------------------------------------------

#[test]
fn scenario_clean_diff_close_proceeds() {
    let envs = vec![
        envelope("correctness", vec![]),
        envelope("testing", vec![]),
        envelope("maintainability", vec![]),
        envelope("project-standards", vec![]),
    ];
    let r = run_pipeline(envs);

    assert!(r.autofix.residual.is_empty());
    assert!(r.autofix.pre_existing.is_empty());
    assert_eq!(r.autofix.exit_reason, ExitReason::NothingToFix);
    assert!(r.route.actions.is_empty());
    assert_eq!(r.decision, GateDecision::Allow);
    assert!(r.envelope_json.contains("\"residual\":[]"));
}

// --------------------------------------------------------------------------
// Scenario 2: safe_auto P3 → autofix applies it → close clean
// --------------------------------------------------------------------------

#[test]
fn scenario_safe_auto_p3_autofix_applies_close_proceeds() {
    let envs = vec![envelope(
        "correctness",
        vec![finding(
            "Unused import",
            FindingSeverity::P3,
            "src/foo.rs",
            1,
            0.85,
            Owner::ReviewFixer,
            AutofixClass::SafeAuto,
            false,
        )],
    )];
    let r = run_pipeline(envs);

    // The autofix loop should have run exactly one round and emptied
    // the residual.
    assert_eq!(r.autofix.rounds_run, 1);
    assert!(r.autofix.residual.is_empty());
    assert_eq!(r.autofix.applied_total.len(), 1);
    // No follow-up tasks because the residual is empty.
    assert!(r.route.routed_ids().is_empty());
    assert_eq!(r.decision, GateDecision::Allow);
}

// --------------------------------------------------------------------------
// Scenario 3: manual P1 → routed to follow-up task → close still proceeds
// --------------------------------------------------------------------------

#[test]
fn scenario_manual_p1_routes_to_followup_close_proceeds() {
    let envs = vec![envelope(
        "correctness",
        vec![finding(
            "Concurrency hazard in retry loop",
            FindingSeverity::P1,
            "src/worker.rs",
            120,
            0.85,
            Owner::DownstreamResolver,
            AutofixClass::Manual,
            false,
        )],
    )];
    let r = run_pipeline(envs);

    // Manual findings are not safe_auto, so the autofix loop exits
    // immediately with NothingToFix and the finding stays in residual.
    assert_eq!(r.autofix.exit_reason, ExitReason::NothingToFix);
    assert_eq!(r.autofix.residual.len(), 1);
    // Review-to-task should have created exactly one follow-up.
    assert_eq!(r.route.created_ids().len(), 1);
    let draft = &r.route.drafts[0];
    assert!(draft.labels.iter().any(|l| l == "severity:P1"));
    assert!(draft.labels.iter().any(|l| l == "code-review"));
    // Gate is permissive on P1 — close proceeds.
    assert_eq!(r.decision, GateDecision::Allow);
}

// --------------------------------------------------------------------------
// Scenario 4: P0 → gate hard-blocks → block message names file/line
// --------------------------------------------------------------------------

#[test]
fn scenario_p0_hard_blocks_close_with_actionable_error() {
    let envs = vec![envelope(
        "security",
        vec![finding(
            "SQL injection in login()",
            FindingSeverity::P0,
            "src/auth.rs",
            42,
            0.95,
            Owner::Human,
            AutofixClass::Manual,
            false,
        )],
    )];
    let r = run_pipeline(envs);

    // Residual carries the P0.
    assert_eq!(r.autofix.residual.len(), 1);
    assert_eq!(r.autofix.residual[0].severity, FindingSeverity::P0);

    // Gate blocks.
    let blocking = match &r.decision {
        GateDecision::BlockOnP0(b) => b.clone(),
        other => panic!("expected BlockOnP0, got {other:?}"),
    };
    assert_eq!(blocking.len(), 1);

    // Block message must be actionable: name the file:line, the title,
    // the why, and the supervisor-override escape hatch.
    let msg = format_block_message("cas-test-001", &blocking);
    assert!(msg.contains("cas-test-001"));
    assert!(msg.contains("src/auth.rs:42"));
    assert!(msg.contains("SQL injection in login()"));
    assert!(msg.contains("bypass_code_review=true"));

    // The follow-up task system should still have routed the P0 (the
    // close gate's job is to BLOCK the close, but a follow-up task is
    // also created so the issue has a tracking record).
    assert_eq!(r.route.created_ids().len(), 1);
    let draft = &r.route.drafts[0];
    assert!(draft.labels.iter().any(|l| l == "severity:P0"));
}

// --------------------------------------------------------------------------
// Scenario 5: multi-persona overlap → merge dedupes + boosts + partitions
// --------------------------------------------------------------------------

#[test]
fn scenario_multi_persona_overlap_merges_and_partitions_correctly() {
    // Two personas hit the same P1 issue at adjacent lines.
    let shared_title = "Unwrap on parsed int can panic";
    let correctness = envelope(
        "correctness",
        vec![finding(
            shared_title,
            FindingSeverity::P1,
            "src/parser.rs",
            42,
            0.80,
            Owner::ReviewFixer,
            AutofixClass::Manual,
            false,
        )],
    );
    let adversarial = envelope(
        "adversarial",
        vec![finding(
            shared_title,
            FindingSeverity::P1,
            "src/parser.rs",
            44,
            0.75,
            Owner::ReviewFixer,
            AutofixClass::Manual,
            false,
        )],
    );
    // A pre-existing P0 from project-standards. Must NOT block the gate.
    let project_standards = envelope(
        "project-standards",
        vec![finding(
            "Pre-existing missing rule compliance",
            FindingSeverity::P0,
            "src/legacy.rs",
            10,
            0.95,
            Owner::Human,
            AutofixClass::Advisory,
            true,
        )],
    );

    let r = run_pipeline(vec![correctness, adversarial, project_standards]);

    // Merge dedupes the shared P1 down to one residual entry.
    let pr_findings: Vec<&Finding> = r
        .autofix
        .residual
        .iter()
        .filter(|f| f.title == shared_title)
        .collect();
    assert_eq!(pr_findings.len(), 1);
    // Cross-reviewer boost applied: max(0.80, 0.75) + 0.10 = 0.90.
    assert!((pr_findings[0].confidence - 0.90).abs() < 1e-5);

    // Pre-existing P0 was partitioned into pre_existing and stays
    // there even though the autofix loop processed the cluster.
    assert_eq!(r.autofix.pre_existing.len(), 1);
    assert!(r.autofix.pre_existing[0].pre_existing);

    // Gate must allow despite the pre-existing P0 being in the
    // envelope (separate list, never blocking).
    assert_eq!(r.decision, GateDecision::Allow);
    assert!(r.envelope_json.contains("\"pre_existing\":["));
}

// --------------------------------------------------------------------------
// Scenario 6: defense in depth — pre_existing P0 in residual still allows
// --------------------------------------------------------------------------

#[test]
fn scenario_pre_existing_p0_in_residual_does_not_block() {
    // Hand-crafted residual where a pre_existing=true P0 has somehow
    // ended up in the residual list (real pipeline filters this earlier;
    // this test pins the gate-level guarantee).
    let envelope = ReviewOutcome {
        residual: vec![{
            let mut f = finding(
                "Old SQL injection on legacy path",
                FindingSeverity::P0,
                "src/legacy.rs",
                99,
                0.95,
                Owner::Human,
                AutofixClass::Manual,
                true, // pre_existing
            );
            f.requires_verification = true;
            f
        }],
        pre_existing: vec![],
        mode: "autofix".to_string(),
        execution: Some(cas_types::ReviewExecution {
            personas_run: 4,
            personas_failed: Vec::new(),
            skipped_reason: None,
            required_personas_missing: Vec::new(),
        }),
    };

    let json = serde_json::to_string(&envelope).unwrap();
    let parsed: ReviewOutcome = serde_json::from_str(&json).unwrap();
    parsed.validate().unwrap();

    assert_eq!(evaluate_gate(&parsed.residual), GateDecision::Allow);
}

// --------------------------------------------------------------------------
// Round-trip and shape contract tests for the envelope itself
// --------------------------------------------------------------------------

#[test]
fn envelope_round_trip_preserves_all_fields() {
    let envs = vec![envelope(
        "correctness",
        vec![finding(
            "Y",
            FindingSeverity::P2,
            "src/x.rs",
            1,
            0.9,
            Owner::ReviewFixer,
            AutofixClass::Manual,
            false,
        )],
    )];
    let r = run_pipeline(envs);
    let parsed: ReviewOutcome = serde_json::from_str(&r.envelope_json).unwrap();
    assert_eq!(parsed, r.envelope);
}

#[test]
fn envelope_validation_rejects_empty_mode() {
    let bad = ReviewOutcome {
        residual: vec![],
        pre_existing: vec![],
        mode: "   ".to_string(),
        execution: None,
    };
    assert!(bad.validate().is_err());
}
