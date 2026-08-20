---
name: cas-tdd
description: Test-driven development discipline for behavior-focused, scoped Cassy changes. Use when a task requires test-first work, red-green-refactor, or integration-test design.
managed_by: cas
---

# Test-Driven Development

Imported and adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock.

Use a red → green loop to produce tests worth keeping. Test behavior through public interfaces, name the observable capability, and choose seams before writing the test. Keep durable decisions and task evidence in Cassy through `mcp__cs__task`, `mcp__cs__spec`, and `mcp__cs__memory`; do not create parallel tracker or context files.

## Seams and slices

- Agree the public seam before testing. A seam is the boundary where a caller observes behavior without reaching into internals.
- Work vertical tracer bullets: one test, the smallest implementation that makes it pass, then the next learned slice. Do not write a horizontal wall of imagined tests.
- Expected values come from an independent source of truth: a worked example, specification, known-good literal, or external contract.
- Use the project’s scoped test command and record the actual proof result in the task. Do not treat a zero-test success as proof.

When module shape or a seam is unclear, consult `cas-codebase-design` for module, interface, depth, seam, adapter, leverage, and locality vocabulary.

## Anti-patterns

- **Implementation-coupled:** tests private methods, verifies a side channel rather than the interface, or breaks under a behavior-preserving refactor.
- **Tautological:** the assertion recomputes expected output with the same logic as production code.
- **Horizontal slicing:** tests all anticipated behavior before any implementation, so the suite becomes insensitive to what actually ships.

## Mocking at real boundaries

Mock external systems, time, randomness, and selected filesystem/network boundaries when a real fixture is unsuitable. Prefer real in-process behavior for code you own; do not mock a collaborator merely to prove it was called.

**NestJS carve-out:** `Test.createTestingModule` provider overrides and mocks are the framework-sanctioned seam for isolating a Nest module’s injected dependencies. They are not an implementation-coupled anti-pattern when the test exercises the module’s public behavior and the override represents a real external or separately-owned provider boundary.

## Loop rules

1. Make the test fail for the intended missing behavior before writing production code.
2. Add only enough code to make that slice pass; do not speculate into future slices.
3. Refactor after behavior is green, retaining the public-behavior contract.
4. Before handoff, run the affected scoped test target and report its nonzero test count and exit status in the Cassy task.
