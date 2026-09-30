---
name: cas-tdd
description: Use when a task requires test-first work, red-green-refactor, seam selection for tests, or integration-test design.
license: MIT
metadata:
  managed_by: cas
---

# Test-Driven Development

Imported and adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock.
See [LICENSE](LICENSE) for the upstream permission notice.

Use a red → green loop to produce tests worth keeping. Keep durable decisions and task evidence in Cassy through `task`, `spec`, and `memory`; do not create parallel tracker or context files.

## Seams and slices

- Agree the public seam before testing. A seam is the boundary where a caller observes behavior without reaching into internals.
- Work vertical tracer bullets: one test, the smallest implementation that makes it pass, then the next learned slice. Do not write a horizontal wall of imagined tests.
- Use the project’s scoped test command and record the actual proof result in the task. Do not treat a zero-test success as proof.
- Factory workers run targeted Rust tests under `cas-worker` discipline: one package, a mandatory named-test `-E` filter, through the capped runner. Commit and run the failing test, then commit the fix and run the same filter for green proof. The supervisor's `ASSEMBLY_PROOF` covers the full suite at epic assembly; an older runtime without this exception requires naming the deferred run. Non-Rust suites still run in the worker.

When module shape or a seam is unclear, consult `cas-codebase-design` for module, interface, depth, seam, adapter, leverage, and locality vocabulary.
For a test that passes without exercising behavior, use [principles.md](../cas-codebase-design/references/principles.md).

## Tests worth keeping

Load [tests.md](references/tests.md) when choosing or reviewing a test. It covers observable contracts, independent expectations, Rust store/Git fixtures, guarded env/cwd, hub-web Vitest examples, unsafe-behavior pins and source/prose audits.

## Mocking at real boundaries

Load [mocking.md](references/mocking.md) when shaping a dependency or its fixture. It defines system boundaries, per-operation ports, dependencies to keep real and framework provider overrides.

## Loop and review

1. Make the test fail for the intended missing behavior before writing production code.
2. Add only enough code to make that slice pass; let its result choose the next slice.
3. Hand the green diff and scoped proof to review. Refactoring belongs to the review stage, outside the implementer's red → green loop; preserve the behavioral tests while improving the design.
