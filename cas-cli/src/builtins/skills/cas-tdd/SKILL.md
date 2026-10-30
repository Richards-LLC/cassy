---
name: cas-tdd
description: Use when writing, trimming or reviewing tests, including test-first work, red-green-refactor, seam selection, or integration-test design.
license: MIT
metadata:
  managed_by: cas
---

# Test-Driven Development

## Fewest tests

Elegant tests, not copious: a test exists to catch a real regression, and each extra one costs CI time and another chance to flake.

- Write the fewest that fail without the change: a few tests that each prove one behaviour, through the real path its caller uses. Never set a test count or a fixture per case.
- `test-first` is met by an added or changed case in any test file plus a recorded failing-first run; it never needs a new file.
- Extend an existing test or table before adding a file; delete tests the change makes redundant or that cover dead code.
- Assert observable behaviour. A test restating the implementation (mirroring its branches or constants) is bloat; mark a durable exact-text contract `// pin: <reason>`.
- No exhaustive matrices, near-duplicates, copied fixtures, sleeps, timing assumptions or host state.
- Claim, identity and state-machine changes are the exception: cover the combination table, every identity kind × every entry kind, as rows of one table-driven test. One happy-path fixture hides the pairs that break.
- Every bug fix adds a regression case that runs red before the fix, so the suite keeps the bug out; where the project keeps a user-flow case catalog, add the case there too.
- Stress loops and live probes are proof artifacts for the task note, not suite members.
- A flaky test is a bug in the test: fix or delete it, never retry-wrap it.
- Each test has one command and lane, selected from the dependency graph, and a failure that names whose fault it is.

Imported and adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock.
See [LICENSE](LICENSE) for the upstream permission notice.

Use a red → green loop to produce tests worth keeping. Keep durable decisions and task evidence in Cassy through `task`, `spec`, and `memory`; do not create parallel tracker or context files.

## Seams and slices

- Agree the public seam before testing. A seam is the boundary where a caller observes behavior without reaching into internals.
- Work vertical tracer bullets: make one test fail for the missing behaviour, add only enough code to pass it, and let its result choose the next slice. Do not write a horizontal wall of imagined tests.
- Record the scoped command's result in the task; a zero-test success is not proof.
- Factory workers run targeted tests under [`cas-worker` discipline](../cas-worker/references/discipline.md): commit and run red, then commit the fix and run that filter green.

With a merge queue, the queue owns the full suite; without one, the full suite must be green on the exact SHA reaching the default branch, via CI check-runs on that SHA (preferred) or the supervisor's integrated run recorded on the epic; workers run targeted tests only. Read existing receipts; each check runs once.

When module shape or a seam is unclear, consult `cas-codebase-design` for module, interface, depth, seam, adapter, leverage, and locality vocabulary.
For a test that passes without exercising behavior, use [principles.md](../cas-codebase-design/references/principles.md).

## Tests worth keeping

Load [tests.md](references/tests.md) when choosing or reviewing a test. It covers observable contracts, independent expectations, Rust store/Git fixtures, guarded env/cwd, hub-web Vitest examples, unsafe-behavior pins and source/prose audits.

## Mocking at real boundaries

Load [mocking.md](references/mocking.md) when shaping a dependency or its fixture. It defines system boundaries, per-operation ports, dependencies to keep real and framework provider overrides.

## Review

Hand the green diff and scoped proof to review. Refactoring belongs to the review stage, outside the red → green loop; preserve the behavioral tests while improving the design.
