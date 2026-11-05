# Factory Worker — Discipline

The shared launch contract in `cas-pty` carries task startup, sequential
ownership, no-foreground-blocking, context headroom and lifecycle rules. This file
covers capped check/test evidence and non-Rust suites without restating those rules.

## Scope and test design

Run the tests that cover the code you touched; name each file or filter you ran.
<!-- cas-src:begin -->
`CONTRIBUTING` names the scoped commands.
<!-- cas-src:else -->
The project's CLAUDE.md or AGENTS.md names the scoped commands.
<!-- cas-src:end -->
Use [Fewest tests](../../cas-tdd/SKILL.md#fewest-tests) and
[Test hygiene](../../cas-tdd/SKILL.md#test-hygiene) for test design and failing-first cases; scope determines existing tests to run, not new
tests to add. Docs-only and no-code tasks prove their acceptance check instead
(e.g. markdownlint, `cas codemap status`, a published artifact); don't add code
tests to them. Record non-Rust exit status and actual test count; zero tests
are not a pass. Report a subset by what it ran (`412 tests, 38 files`), never as
"the full suite": an audited lane called 221 files the suite, hiding every file it skipped.

## Check the committed change before parking

<!-- cas-src:begin -->
Workers may run exactly `cargo check -p <crate> [-p <crate> ...] --lib` or
the same command with `--tests`. Choose `--lib` for lib-only edits and `--tests`
when test files changed; never combine the target flags.
Select the affected crates, including consumers of changed shared interfaces.
Commit first, then background long checks with a log in the ignored target directory:

```bash
mkdir -p target
cargo check -p affected-crate --tests > target/worker-check.log 2>&1 &
```

The guard runs a backgrounded check as `worker-check --detach`, which starts
the runner in its own session and returns, because Codex kills a shell `&` job
when the call's shell exits. A backgrounded check without a log is refused.
The shell creates the log before admission checks the commit. Logs inside the
repository must be Git-ignored, such as `target/worker-check.log`; a bare
`worker-check.log` is refused. Task artifact directories remain valid outside
the checkout. Create the parent directory before invoking the check.

The PreToolUse hook routes this command through the capped runner. That applies to
Claude's Bash and to Codex's `exec_command`, including calls made from code mode
(`functions.exec`). The runner uses your private seeded target cache, holds a
builder slot for the Cargo process lifetime, and refuses when the existing build
guard or `max_concurrent_builders` cap is exceeded. Retry later after a refusal;
do not bypass it with a toolchain, environment override, shell wrapper, broader
flags, or another command. If a check ever runs as raw `cargo` (no runner
process, no `check: PASS` line), stop it and report a blocker rather than
running Cargo another way.

On success the runner records `check: PASS <sha>` against the clean commit.
Close copies that receipt into task notes when it matches the delivered SHA.
A changed or dirty tree needs a new check. `--tests` also compiles test code;
neither shape executes tests or replaces the supervisor's assembly proof.

Run targeted Rust tests through the same capped runner:

```bash
cargo nextest run -p affected-crate --lib -E 'test(module::name)' > target/worker-tests.log 2>&1 &
```

Select exactly one package and a mandatory positive named-test filterset:
`test(name)` or `test(=module::name)`, optionally joined by `|` or `&`.
Select `--lib` or one `--test <harness>` from the project's explicit Cargo
harness inventory (`scripts/cas-test-targets.py` in cas-src). An omitted target
means `--lib`. Empty, `all()`, glob/regex and negated filters, repeated packages,
broad flags, environment prefixes and compound commands are refused.
Commit first; zero matched tests fail. Success records
`test: PASS <sha> <package> <filter> <count>`; close imports exact-delivery
receipts. Run the red test before fixing it, then commit and run the green test.

Rust example: `--lib -E 'test(close_ops::changed_fn)'`.
<!-- cas-src:else -->
In a Rust project, Cassy's capped runner admits only
`cargo check -p <crate> --lib` (or `--tests`) and
`cargo nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)'`, each
committed first and backgrounded with a Git-ignored log; the hook's refusal names
the exact shapes. Otherwise run the project's own targeted test commands
directly and record their exit status and test count.
<!-- cas-src:end -->
Before rewording user-facing text, grep the tests for the old wording. Measure
a size or count claim on a real tree as well as a fixture. List each module run
in the close reason.

Write or update Rust tests, read the diff, and trace callers, struct literals,
and match arms across consumers. Before the push, cut the tests you added to the
[fewest that fail without the change](../../cas-tdd/SKILL.md#fewest-tests). Full Rust builds and suites, cargo test,
clippy, rustc, and scoped-test scripts remain supervisor-owned. Do not record
a scoped `--proof`,
`SCOPED_PROOF:` or `loaded_proof` receipt. If the installed runtime predates the
targeted-test exception, park and name the unverified crates for the supervisor.

<!-- cas-src:begin -->
Workers run only the tests covering the lines they touched, before park. The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. QA walks impacted journeys once per release batch. Read receipts; never rerun a passing check.
<!-- cas-src:else -->
Workers run only the tests covering the lines they touched, before park. The full suite runs once on the landing tree: in CI on that SHA, else one supervisor run per epic. QA walks impacted journeys once per release batch. Read receipts; never rerun a passing check.
<!-- cas-src:end -->
For a supervisor's integrated run, record
`ASSEMBLY_PROOF: head=<tested sha> result=PASS command=<cmd> log=<path>` on
the epic. Child closes reference the epic's integration proof; final-gate
failures return as follow-up tasks.

## Capacity waits

For a capped refusal caused by builder slots, CPU/memory pressure or disk
headroom, keep the task InProgress and retain its lease. Follow the recorded
next retry time and retry the same command; do not set status=blocked or send a
supervisor blocker page for capacity alone. Record progress if useful, arm one
retry reminder, then end the turn. The runner records one capacity-wait episode
shown by worker_status; admission clears it. The daemon can queue one notice per
episode after 20 minutes, including when no retry occurs; delivery follows normal
supervisor readiness policy. Report an actual need for supervisor action
immediately through the ordinary blocker message path.
A source/test failure or a required decision still follows its own workflow.

## Browser checks

Browser suites use the same tiering as Rust: workers and independent QA run
vitest, tsc and source-impact-selected journeys.
<!-- cas-src:begin -->
Where available, use
`scripts/journey-eval.sh <task-artifact-dir>`: default task-target selection,
four workers, explicit empty receipt when nothing is affected. Pass
`--affected <base>` to bind an explicit ancestor base. Arbitrary spec/grep
filters cannot substitute for the producer's selection.
<!-- cas-src:else -->
Select the journeys the diff affects with the project's own test commands.
<!-- cas-src:end -->
Manual named-spec or
canonical-ID runs remain available for iteration and failure controls.

The hook denies factory workers/reviewers `--full`, unfiltered Playwright and
unfiltered journey npm scripts. Full-suite ownership follows the rule above; close
and QA do not require a journey receipt. Documentation/ledger-only rebinds must
prove product/catalog/tool inputs are unchanged. A failure control runs only
the failing spec with one worker and keeps the original failure; it never
reports that failure as PASS.

## Clean-CI environment

Factory shells export `CAS_*` identity variables. When a diff touches agent
resolution, coordination, messaging, cloud config, or another environment-sensitive
path, name it in the close note; assembly uses the project's clean-environment
wrapper if available. `CAS_ROOT` and `CAS_CLONE_PATH` can redirect a test to the
main checkout's `.cas`. There is no `CAS_TASK_ID`.
