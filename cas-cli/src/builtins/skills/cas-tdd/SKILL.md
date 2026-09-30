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

Use a red → green loop to produce tests worth keeping. Test behavior through public interfaces, name the observable capability, and choose seams before writing the test. Keep durable decisions and task evidence in Cassy through `task`, `spec`, and `memory`; do not create parallel tracker or context files.

## Seams and slices

- Agree the public seam before testing. A seam is the boundary where a caller observes behavior without reaching into internals.
- Work vertical tracer bullets: one test, the smallest implementation that makes it pass, then the next learned slice. Do not write a horizontal wall of imagined tests.
- Expected values come from an independent source of truth: a worked example, specification, known-good literal, or external contract.
- Use the project’s scoped test command and record the actual proof result in the task. Do not treat a zero-test success as proof.
- Factory workers run targeted Rust tests under `cas-worker` discipline: one package, a mandatory named-test `-E` filter, through the capped runner. Commit and run the failing test, then commit the fix and run the same filter for green proof. The supervisor's `ASSEMBLY_PROOF` covers the full suite at epic assembly; an older runtime without this exception requires naming the deferred run. Non-Rust suites still run in the worker.

When module shape or a seam is unclear, consult `cas-codebase-design` for module, interface, depth, seam, adapter, leverage, and locality vocabulary.
For a test that passes without exercising behavior, use [principles.md](../cas-codebase-design/references/principles.md).

## Tests worth keeping

A test describes observable behavior through a public interface and survives an internal refactor. Name the observable capability, not the implementation steps. One logical capability per test keeps a failure legible; a small table can compare independent cases sharing a public contract.

- **Implementation-coupled:** tests private methods, mocks internal collaborators, verifies a persistence side channel instead of the interface, or breaks under a behavior-preserving refactor.
- **Tautological:** recomputes the expected value with production's algorithm, asserts a constant equal to itself, or hand-derives a snapshot with the same construction logic. A constant compared with its restated literal is a change detector, not proof that callers observe the promised behavior.
- **Horizontal slicing:** writes all anticipated tests before learning from any implementation. Work one observed capability at a time.

Load [tests.md](references/tests.md) when choosing or reviewing a test: it gives worked good/bad examples and the audit patterns for reading `.rs` source as text, asserting line order in source, and prose pins. Preserve intentional contract checks with a reason; use the project's test-shape lint rather than adding reminders.

## Mocking at real boundaries

Mock external systems, time, randomness, and selected filesystem/network/database boundaries when a real fixture is unsuitable. Prefer a real test database for code you own. Keep mocks specific to a boundary's contract; never mock an internal collaborator merely to prove it was called.

Use SDK-style per-operation interfaces at external seams, so each mock supplies one typed response without routing by URL or method. Load [mocking.md](references/mocking.md) when shaping the dependency or its test fixture.

A framework's own testing seam (a DI container's provider override, a test harness's module builder) is not an implementation-coupled mock when the test still asserts the module's public behavior and the override stands in for a real external or separately-owned dependency.

## Loop and review

1. Make the test fail for the intended missing behavior before writing production code.
2. Add only enough code to make that slice pass; let its result choose the next slice.
3. Hand the green diff and scoped proof to review. Refactoring belongs to the review stage, outside the implementer's red → green loop; preserve the behavioral tests while improving the design.
