---
name: cas-tdd
description: Use when a task requires test-first work, red-green-refactor, seam selection for tests, or integration-test design.
managed_by: cas
---

# Test-Driven Development

Imported and adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock.

Use a red → green loop to produce tests worth keeping. Test behavior through public interfaces, name the observable capability, and choose seams before writing the test. Keep durable decisions and task evidence in Cassy through `cas__task`, `cas__spec`, and `cas__memory`; do not create parallel tracker or context files.

## Seams and slices

- Agree the public seam before testing. A seam is the boundary where a caller observes behavior without reaching into internals.
- Work vertical tracer bullets: one test, the smallest implementation that makes it pass, then the next learned slice. Do not write a horizontal wall of imagined tests.
- Expected values come from an independent source of truth: a worked example, specification, known-good literal, or external contract.
- Use the project’s scoped test command and record the actual proof result in the task. Do not treat a zero-test success as proof.

When module shape or a seam is unclear, consult `cas-codebase-design` for module, interface, depth, seam, adapter, leverage, and locality vocabulary.

## Tests worth keeping

A test describes observable behavior through a public interface and survives an internal refactor. Name what a caller can do, not which helper happened to run. One logical capability per test keeps a failure legible; a small table is useful when independent cases share a public contract, but it is not a substitute for behavior-focused names.

Three shapes to reject:

- **Implementation-coupled:** tests private methods, asserts call counts or ordering, verifies a persistence side channel rather than the interface, or breaks under a behavior-preserving refactor.
- **Tautological:** the assertion recomputes the expected output with the same logic as production code. Take the expected value from a known literal, a worked example, or a specification instead.
- **Horizontal slicing:** tests all anticipated behavior before any implementation, so the suite becomes insensitive to what actually ships.

## Mocking at real boundaries

Mock external systems, time, randomness, and selected filesystem, network, or database boundaries when a real fixture is unsuitable. Keep each mock specific to that boundary's contract, so a failing test names the behavior that changed. Prefer real in-process behavior for code you own; do not mock an internal collaborator merely to prove it was called.

A framework's own testing seam (a DI container's provider override, a test harness's module builder) is not an implementation-coupled mock when the test still asserts the module's public behavior and the override stands in for a real external or separately-owned dependency.

## Loop rules

1. Make the test fail for the intended missing behavior before writing production code.
2. Add only enough code to make that slice pass; do not speculate into future slices.
3. Refactor after behavior is green, retaining the public-behavior contract.
4. Before handoff, run the affected scoped test target and report its nonzero test count and exit status in the Cassy task.
