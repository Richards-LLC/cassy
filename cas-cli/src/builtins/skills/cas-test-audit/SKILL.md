---
name: cas-test-audit
description: Use when asked to audit a project's tests for dead, duplicate, style-pinning, slow or flaky tests, or to cut test cost; not for writing one new test (cas-tdd).
metadata:
  managed_by: cas
---

# Test value audit

Produce a read-only ledger of which tests protect nothing, which cost too much, and what a cut would save. Change no test while auditing.

**The rule:** audit first, cut only after operator approval, every cut names its covering test (or says `none needed: dead` / `none needed: superseded`). Cuts ship as a separate approved task per batch, each with a before/after suite-time check.

## What the measurements say

Read this before promising speed. In cas-src (the reference audit, 10 green integration runs):

- Every approved delete and merge removed about 13 test-seconds from a run of 2,906. Deleting tests is about correctness, not speed.
- The slowest 5% of Rust tests held 68% of test time, and runner config was the biggest win in a second project (Jest 2 workers: 7.4 to 3.6 min). Fixing a few slow tests and runner levers beats deleting many.
- In 12 later queue runs the median was 20.3 min wall. The critical path was the Rust archive build (about 14 min, not a test), then the Rust shards (about 6 min). Halving the slowest 137 tests saved about 44 s per shard. A test cost list is not the route to queue speed; report the build as the largest block and send speed work to the CI owner.

## Steps

1. **Scope.** Name the project, the code revision and the runners. Done when the ledger header states them. Work one project at a time; use read-only fan-out, one agent per bucket, never a full local suite.
2. **Baseline cost.** Follow [ci-timing](references/ci-timing.md) for each runner. Done when a table lists tests, test-seconds, shard wall and thread use per layer, and the CI run ids used. If a project keeps no CI artifacts, write `not measurable`; never estimate.
3. **Run the detectors** in [detectors](references/detectors.md) for the languages present. Each detector reports flagged and, after a 10-row hand check, held. Done when every detector row has `held / flagged`.
4. **Apply the precision floor.** A detector under 50% on its sample goes in the ledger as `noisy, not proposed`. Its rows are never proposed for deletion. Low-confidence rows are listed, never proposed for deletion.
5. **Classify** every proposal into one bucket from the table below, with a confidence (`high` tool-authoritative or both sides quoted, `medium`, `low`).
6. **Write the ledger** in [ledger](references/ledger.md). Keep measured time removed and projected saving in separate columns; a saving is a projection until a before/after run proves it.
7. **Stop.** Hand the ledger to the operator with the decisions list. Done when no test file differs from the audited revision.

## Buckets

| bucket | meaning |
|---|---|
| dead | tests of code production never calls |
| superseded | tests of a retired behaviour or alias, with the retiring commit cited |
| restates-implementation | the test copies the logic it checks, or pins a constant, snapshot or source text |
| duplicate | another test asserts the same behaviour; quote both assertions |
| slow | a real sleep, per-test rebuilt setup or serial loop; fix the cause, delete only if another bucket also flags it |

Style pinning (CSS or source-text assertions) is deleted, not updated: Visual QA and behavioural tests own appearance. Keep the DOM or behaviour assertions beside it.

## Two lists, not one score

- **Value list** (cut or merge): highest-confidence flag first.
- **Cost list** (speed): `runtime x runs + flake_rate x rerun_cost`, divided by unique coverage; a test with no unique coverage ranks as pure cost. Flake rate and unique coverage are measured inputs: flake from CI retry data, unique coverage from assertion-to-assertion mapping in the same module. Line-coverage diffing only where the project already produces reports.
- **Runner levers:** measured thread use, worker count, shard imbalance. One A/B run proves a change.

## Hard limits

- No gate on a detector until its precision is measured high (at least 90% on its sample) and the operator approves. A noisy check that blocks correct work is worse than none.
- A cut test file that another module imports is not a cut candidate; check imports before proposing.
- Do not recount a test as dead because only test code calls it; wrappers used only by tests stay, move them under the test cfg.
