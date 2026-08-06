//! Latency benchmark for the cas-code-review deterministic post-LLM
//! pipeline (cas-22fa).
//!
//! Run with: `cargo bench --bench code_review_pipeline`
//!
//! ## What this measures
//!
//! The full *deterministic* portion of the close-time code-review gate:
//!
//!   merge_findings → autofix_loop (zero-progress short-circuit)
//!     → route_residual_to_tasks → ReviewOutcome JSON serialize
//!     → ReviewOutcome JSON deserialize + validate
//!     → evaluate_gate → final GateDecision
//!
//! ## What this does NOT measure
//!
//! The seven persona LLM dispatches and the orchestrator's prompt
//! roundtrip. Those are network/model-bound, not Rust-bound, and any
//! benchmark of them would be measuring Anthropic's API latency, not
//! cas. The brainstorm's ~90s pressure threshold is a *whole-gate*
//! budget; the deterministic core measured here is the part of that
//! budget that cas owns and can optimize.
//!
//! Use the printed numbers as the floor of the whole-gate budget. If
//! this benchmark shows the deterministic core taking more than a few
//! milliseconds at 100 LOC equivalent (≈10 findings), something is
//! wrong before you even start asking about LLM time.
//!
//! ## Workload sizes
//!
//! The brainstorm uses LOC as the proxy. We translate to *finding count*
//! because the deterministic pipeline scales with findings, not lines:
//!
//!   - 10 findings  ≈  ~10 LOC change with 1 finding/LOC saturation
//!   -100 findings  ≈  ~100 LOC change
//!   -500 findings  ≈  ~500 LOC change
//!
//! Each workload uses a 7-persona envelope distribution with overlap so
//! the merge stage actually exercises dedup + agreement boost rather
//! than just walking a flat list.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use cas_store::code_review::{
    autofix_loop,
    close_gate::{evaluate_gate, format_block_message},
    merge_findings, route_residual_to_tasks, FixerResult,
};
use cas_types::{AutofixClass, Finding, FindingSeverity, Owner, ReviewOutcome, ReviewerOutput};

/// Generate a synthetic 7-persona envelope set with `n` total findings,
/// roughly 30% overlap to exercise dedup, 60% manual / 30% safe_auto /
/// 10% advisory class mix, and 1% P0 / 9% P1 / 60% P2 / 30% P3 severity
/// mix. Severity is biased so the gate evaluates the realistic
/// "mostly P2/P3 with a sprinkle of P1 and rare P0" production case.
fn generate_envelopes(n: usize) -> Vec<ReviewerOutput> {
    const PERSONAS: &[&str] = &[
        "correctness",
        "testing",
        "maintainability",
        "project-standards",
        "security",
        "performance",
        "adversarial",
    ];
    let mut buckets: Vec<Vec<Finding>> = vec![Vec::new(); PERSONAS.len()];
    for i in 0..n {
        let persona_ix = i % PERSONAS.len();
        let line = (i as u32 % 1000) + 1;
        // Overlap: every 3rd finding shares a title and adjacent line
        // with the previous persona's pick — exercises fingerprint dedup.
        let overlap_pair = i % 3 == 0;
        let title = if overlap_pair {
            format!("Shared issue {}", i / 3)
        } else {
            format!("Unique issue {i}")
        };
        let severity = match i % 100 {
            0 => FindingSeverity::P0,       // 1%
            1..=9 => FindingSeverity::P1,   // 9%
            10..=69 => FindingSeverity::P2, // 60%
            _ => FindingSeverity::P3,       // 30%
        };
        let class = match i % 10 {
            0..=5 => AutofixClass::Manual,
            6..=8 => AutofixClass::SafeAuto,
            _ => AutofixClass::Advisory,
        };
        let owner = match class {
            AutofixClass::SafeAuto => Owner::ReviewFixer,
            AutofixClass::Advisory => Owner::Human,
            _ => Owner::DownstreamResolver,
        };
        let f = Finding {
            title,
            severity,
            file: format!("src/module_{}.rs", i % 50),
            line,
            why_it_matters: "synthetic finding for benchmark".to_string(),
            autofix_class: class,
            owner,
            confidence: 0.75 + ((i % 25) as f32) / 100.0,
            evidence: vec![format!("evidence line {i}")],
            pre_existing: i % 20 == 0, // 5% pre-existing
            suggested_fix: None,
            requires_verification: false,
        };
        buckets[persona_ix].push(f);
    }
    PERSONAS
        .iter()
        .zip(buckets)
        .map(|(name, findings)| ReviewerOutput {
            reviewer: name.to_string(),
            findings,
            residual_risks: vec![],
            testing_gaps: vec![],
            skipped_reason: None,
        })
        .collect()
}

/// Run one full pipeline pass and return the bytes-equivalent work as
/// the gate decision. The `black_box` calls prevent the optimizer from
/// constant-folding away the work between iterations.
fn run_pipeline_once(envelopes: &[ReviewerOutput]) {
    let merged = merge_findings(envelopes.to_vec()).expect("valid envelopes");

    // Autofix with a zero-progress fixer to short-circuit cleanly. The
    // real fixer dispatches an LLM and is not part of the deterministic
    // budget — measuring it here would just measure the FixerResult
    // construction overhead.
    let outcome = autofix_loop(
        merged,
        |_| FixerResult::default(),
        || unreachable!("zero-progress fixer never triggers a rereview"),
    );

    // Route residual to tasks (no persistence; closures are no-ops).
    let _ = route_residual_to_tasks(
        &outcome.residual,
        |_| None,
        |_| Ok("task-bench".to_string()),
        |_, _| Ok(()),
    )
    .expect("route succeeds");

    // Assemble + round-trip envelope.
    let envelope = ReviewOutcome {
        residual: outcome.residual.clone(),
        pre_existing: outcome.pre_existing,
        mode: "autofix".to_string(),
        execution: None,
    };
    let json = serde_json::to_string(&envelope).expect("serialize");
    let parsed: ReviewOutcome = serde_json::from_str(&json).expect("deserialize");
    parsed.validate().expect("validate");

    // Evaluate gate.
    let decision = evaluate_gate(&parsed.residual);
    if let cas_store::code_review::close_gate::GateDecision::BlockOnP0(blocking) = &decision {
        // Touch the block message formatting so it's part of the budget.
        let _ = black_box(format_block_message("cas-bench", blocking));
    }
    black_box(decision);
}

fn bench_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("code_review_pipeline");
    group.sample_size(40);
    group.measurement_time(std::time::Duration::from_secs(6));

    for &n in &[10usize, 100, 500] {
        let envs = generate_envelopes(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &envs, |b, envs| {
            b.iter(|| run_pipeline_once(black_box(envs)));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
