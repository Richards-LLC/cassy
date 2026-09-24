# Operating Discipline — No Rust Builds

The PTY spawn contract is the source of truth for worker availability, long
commands, context limits, and reporting shape. This reference intentionally
does not restate those launch-time rules. It covers the worker build rule and
the clean-CI environment the supervisor's assembly run must respect.

## Workers never run Rust builds

Factory workers do not build or test Rust. A PreToolUse guard denies workers
any `cargo` build/check/test/nextest/clippy/run, `rustc`,
`scripts/run-scoped-tests.sh`, and `make test*`. Edit and commit, then park the
work without building:

- Batch related edits into logical commits. Read your own diff and `rg` every
  caller, struct literal, and match arm a compiler would otherwise flag.
- Write or update the Rust tests the change needs; they run at assembly, not in
  your worktree.
- Do not record a scoped `--proof` receipt, a `SCOPED_PROOF:` note, or a
  `loaded_proof` note. Worker closes no longer need them.
- When the guard denies a build, do not route around it (another wrapper,
  `bash -c`, a script). Park the work.

Only the supervisor builds: once per epic at assembly it runs one full build +
test of the epic tip and records
`ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>` on
the epic. Child task closes reference that proof; an assembly failure comes
back as a follow-up task.

## Non-Rust work is unaffected

Non-Rust suites (for example hub-web `npm`/`vitest`/`playwright`) still run in
the worker. A green exit code is not a green test run: the receipt must show a
harness summary and a nonzero passed count. Record the exact passed and failed
counts in the close note; a zero-test run is a failure to run.

## Clean-CI environment

Factory shells export `CAS_*` identity variables. Tests that read them can pass
locally and fail in clean CI. When a diff touches agent resolution,
coordination, messaging, cloud config, or another environment-sensitive path,
say so in the close note; the supervisor's assembly run then uses the project's
clean-environment wrapper:

```bash
make -C cas-cli test-clean-env
make -C cas-cli test-clean-env CLEAN_ENV_ARGS='--lib cloud::config'
```

The wrapper enumerates and strips the live `CAS_*` variables; do not hand-write
an `env -u` list. In particular, `CAS_ROOT` and `CAS_CLONE_PATH` can redirect a
test to the main checkout's `.cas`. There is no `CAS_TASK_ID`.
