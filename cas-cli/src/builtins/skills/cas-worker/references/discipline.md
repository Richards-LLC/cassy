# Factory Worker — Discipline

Capped check and test evidence, browser suites and capacity waits. The launch
contract carries the lifecycle rules.

## Scope and test design

Run affected targeted tests: the whole test module of each changed production
function plus every integration harness that calls it. `CONTRIBUTING` names the
scoped commands. Use [Fewest tests](../../cas-tdd/SKILL.md#fewest-tests) and
[Test hygiene](../../cas-tdd/SKILL.md#test-hygiene) for test design and failing-first cases; scope determines existing tests to run, not new
tests to add. Docs-only and no-code tasks prove their acceptance check instead
(e.g. markdownlint, `cas codemap status`, a published artifact); don't add code
tests to them. Record non-Rust exit status and actual test count; zero tests
are not a pass. Report a subset by what it ran (`412 tests, 38 files`), never as
"the full suite".

## Check the committed change before parking

Workers may run exactly `cargo check -p <crate> [-p <crate> ...] --lib` or
the same command with `--tests`. Choose `--lib` for lib-only edits and `--tests`
when test files changed; never combine the target flags. Select the affected
crates, including consumers of changed shared interfaces. Commit first, then
background long checks with a Git-ignored log:

```bash
mkdir -p target
cargo check -p affected-crate --tests > target/worker-check.log 2>&1 &
```

The PreToolUse hook routes this through the capped runner, which uses your
private target cache and refuses past the build guard or `max_concurrent_builders`.
After a refusal retry later; never bypass it with another toolchain, environment
override, wrapper, broader flags or command. If a check runs as raw `cargo` (no
`check: PASS` line), stop it and report a blocker. Success records
`check: PASS <sha>` against the clean commit and close imports it; a changed
tree needs a new check. Neither shape executes tests.

Run targeted Rust tests through the same runner, one package and a positive
named-test filter:

```bash
cargo nextest run -p affected-crate --lib -E 'test(module::name)' > target/worker-tests.log 2>&1 &
```

Pick `--lib` or one `--test <harness>` from the project's harness inventory
(`scripts/cas-test-targets.py` in cas-src); the refusal names any rejected
shape. Zero matched tests fail. Run the red test before fixing, then commit and
run the green one. Cover each affected module and harness, for example
`--lib -E 'test(close_ops::)'` plus
`--test integration_factory -E 'test(worktree_surface_test::)'`, and list each
in the close reason: named-only proofs missed same-module assertions and other
callers' tests. Before rewording user-facing text, grep the tests for the old
wording. Measure a size or count claim on a real tree as well as a fixture.

Read the diff and trace callers, struct literals and match arms across
consumers. Before the push, cut the tests you added to the
[fewest that fail without the change](../../cas-tdd/SKILL.md#fewest-tests).
Full Rust builds and suites, cargo test, clippy and scoped-test scripts stay
supervisor-owned; do not record a scoped `--proof`, `SCOPED_PROOF:` or
`loaded_proof` receipt. If the installed runtime predates the targeted-test
exception, park and name the unverified crates.

With a merge queue, the queue owns the full suite; without one, the full suite must be green on the exact SHA reaching the default branch, via CI check-runs on that SHA (preferred) or the supervisor's integrated run recorded on the epic; workers run targeted tests only. Read existing receipts; each check runs once.
A supervisor's integrated run records
`ASSEMBLY_PROOF: head=<tested sha> result=PASS command=<cmd> log=<path>` on the
epic; child closes reference it.

## Capacity waits

A capped refusal for builder slots, CPU/memory pressure or disk headroom is not
a blocker: keep the task InProgress, arm one retry reminder for the recorded
retry time, end the turn and retry the same command. Do not set status=blocked
or page the supervisor for capacity alone; Cassy reports long waits itself.
Report an actual need for supervisor action through the ordinary blocker path.

## Browser checks

Workers and independent QA run vitest, tsc and source-impact-selected journeys.
Where available, use `scripts/journey-eval.sh <task-artifact-dir>`, which picks
the affected journeys and writes an explicit empty receipt when none are;
`--affected <base>` binds an explicit ancestor base. Spec or grep filters cannot
substitute for that selection, though named-spec runs stay available for
iteration. The hook denies workers `--full`, unfiltered Playwright and
unfiltered journey npm scripts. A failure control runs only the failing spec
with one worker and never reports that failure as PASS.

## Clean-CI environment

Factory shells export `CAS_*` identity variables. When a diff touches agent
resolution, coordination, messaging, cloud config or another environment-sensitive
path, name it in the close note so assembly uses the clean-environment wrapper.
`CAS_ROOT` and `CAS_CLONE_PATH` can redirect a test to the main checkout's `.cas`.
