# Operating Discipline — Scoped Verification

The PTY spawn contract is the source of truth for worker availability, long
commands, context limits, and reporting shape. This reference intentionally
does not restate those launch-time rules. It covers test execution details that
belong with the repository's scoped-test wrapper.

## Scoped tests

Workers do not own full-suite runs. A full suite links dozens of test binaries;
iterate by compiling, then prove only the affected target:

- `cargo check -p <crate> --lib --tests` — compile feedback without test runs.
- `scripts/run-scoped-tests.sh -p cas --lib <module>` — one library target or
  filter through nextest.
- `scripts/run-scoped-tests.sh -p cas --test <name>` — one integration test
  file through nextest.
- `scripts/run-scoped-tests.sh --proof ...` — final receipt; it checks that the
  committed test surface is covered.
- `CARGO_CMD=test scripts/run-scoped-tests.sh ...` — diagnostic fallback only.

Package selection alone (`-p cas`) is not a scope here: that package owns many
test binaries. Reserve full runs for supervisor integration and release gates.

## The test loop: inner loop vs final proof

Batch before you verify: group related fixes before running the affected target.
The inner loop is quick
compile feedback; the final proof is the one affected-target run after the
batch and, if needed, one pre-push receipt. Do not spend a full sweep after
each micro-fix.

**Inner loop:** use `cargo check -p <crate> --lib --tests` while editing. It
catches type, borrow, feature, and test-compilation errors without linking or
executing test binaries.

**Final proof:** run the affected target after all fixes, at most twice unless a
new edit changes that target. The equivalent direct command is
`cargo nextest run --lib <filter>`; use the guarded wrapper and read its summary.
Reuse a banked receipt when later edits are outside its blast radius, recording
the commit and covered surface in the task note.

The standard shape is: check while editing → batch fixes → one scoped nextest
run → record the passed count and output tail → push. A background build or
test must be allowed to finish before its result is reported.

## A green exit code is not a green test run

The wrapper's receipt must show a harness summary and a nonzero passed count.
An exit code without a reported test is not proof. Read the `test result:` or
nextest `Summary` line yourself; record the exact passed and failed counts in
the close note. A zero-test run is a failure to run.

```bash
make -C cas-cli test-scoped SCOPED_ARGS='--proof -p cas --lib my_module'
scripts/run-scoped-tests.sh --proof -p cas --test cli_test
```

## Clean-CI environment

Factory shells export `CAS_*` identity variables. Tests that read them can pass
locally and fail in clean CI, so use the project's clean-environment wrapper
when the diff touches agent resolution, coordination, messaging, cloud config,
or another environment-sensitive path:

```bash
make -C cas-cli test-clean-env
make -C cas-cli test-clean-env CLEAN_ENV_ARGS='--lib cloud::config'
```

The wrapper enumerates and strips the live `CAS_*` variables; do not hand-write
an `env -u` list. In particular, `CAS_ROOT` and `CAS_CLONE_PATH` can redirect a
test to the main checkout's `.cas`. There is no `CAS_TASK_ID`.
