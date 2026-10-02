# Factory Worker — Discipline

The shared launch contract in `cas-pty` carries task startup, sequential
ownership, no-foreground-blocking, context headroom and lifecycle rules. This file
covers capped check/test evidence and non-Rust suites without restating those rules.

## Check the committed change before parking

Workers may run exactly `cargo check -p <crate> [-p <crate> ...] --lib` or
the same command with `--tests`. Choose `--lib` for lib-only edits and `--tests`
when test files changed; never combine the target flags.
Select the affected crates, including consumers of changed shared interfaces.
Commit first, then background long checks with a log in the ignored target directory:

```bash
mkdir -p target
cargo check -p affected-crate --tests > target/worker-check.log 2>&1 &
```

The shell creates the log before admission checks the commit. Logs inside the
repository must be Git-ignored, such as `target/worker-check.log`; a bare
`worker-check.log` is refused. Task artifact directories remain valid outside
the checkout. Create the parent directory before invoking the check.

The PreToolUse hook routes this command through the capped runner. It uses your
private seeded target cache, holds a builder slot for the Cargo process lifetime,
and refuses when the existing build guard or `max_concurrent_builders` cap is
exceeded. Retry later after a refusal; do not bypass it with a toolchain,
environment override, shell wrapper, broader flags, or another command.

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

Write or update Rust tests, read the diff, and trace callers, struct literals,
and match arms across consumers. Full Rust builds and suites, cargo test,
clippy, rustc, and scoped-test scripts remain supervisor-owned. Do not record
a scoped `--proof`,
`SCOPED_PROOF:` or `loaded_proof` receipt. If the installed runtime predates the
targeted-test exception, park and name the unverified crates for the supervisor.

The supervisor runs the full build and tests once at epic assembly and records
`ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>` on
the epic. Child closes reference it; assembly failures return as follow-up tasks.

## Non-Rust work is unaffected

Run non-Rust suites (for example `npm`/`vitest`/`playwright`) in the worker.
Record exact passed and failed counts in the close note. A green exit without
a nonzero test count is a failure to run.

## Clean-CI environment

Factory shells export `CAS_*` identity variables. When a diff touches agent
resolution, coordination, messaging, cloud config, or another environment-sensitive
path, name it in the close note; assembly uses the project's clean-environment
wrapper if available. `CAS_ROOT` and `CAS_CLONE_PATH` can redirect a test to the
main checkout's `.cas`. There is no `CAS_TASK_ID`.
