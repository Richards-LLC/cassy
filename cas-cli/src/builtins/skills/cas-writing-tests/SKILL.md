---
name: cas-writing-tests
description: Use when asked to write tests or improve unit/integration test quality, including excellent or kick-ass tests, TDD, fakes and fixtures; browser/e2e/journey authoring uses cas-playwright-tests.
license: MIT
metadata:
  managed_by: cas
---

# Writing tests

## Fewest tests

Choose the public seam where the caller observes the promised result. Extend an existing case or table before adding a file; keep the fewest cases that fail without the guarded behavior. Cover identity/state combinations as rows when a single happy path hides the incident. Delete obsolete or duplicate tests after checking their importers.

For browser/e2e/journey authoring, use [cas-playwright-tests](../cas-playwright-tests/SKILL.md); diagnose failures with `cas-playwright-debug`, and audit an existing suite with `cas-test-audit`.

## Test hygiene

1. Prove a new or changed test red by reverting the fix or deleting the guarded line. Sabotage must break the promised outcome through its real consumer; an unrelated timeout proves nothing. Keep the red command/result, restore the code and record green.
2. Assert the caller's outcome using independent fixture/source values or stable protocol identifiers. Banned: copy, word lists, punctuation, source-code text and implementation/style pins. Exact caller payloads are data-integrity contracts; fixed safety bytes need a named consumer. Browser accessibility has one narrow computed-style exception in the browser skill.
3. Make fakes evaluate the same predicates as the dependency. Keep uniqueness, transactions, locking and Git ancestry on real disposable databases/repos; validate the disposable target before setup or cleanup. A canned success/count can hide races.
4. Control clocks only for time-dependent logic, before the code reads time. Use bounded event/completion barriers for real processes and IO; do not replace synchronization with sleeps.
5. Fail when required fixture/auth/data is missing. A declared optional lane may skip with a visible reason; a soft return is a false pass. Execute known defects as expected failures and fail on an unexpected pass.
6. Read the runner's executed count and CI coverage. Fail globally empty required selections, unexplained skips or missing required CI lanes; declared optional lanes report their skip. Shards may be empty individually when the aggregate executes required coverage. A test outside every CI lane needs an explicit owner/decision, not a green claim.
7. Exercise owned behavior, not the library/runtime. Prove dependency/runtime upgrades with existing relevant suites; remove unused dependencies. Keep imports inside tests when they provide module/mock isolation, and share only immutable setup.
8. Fix or delete a flaky test. Banned: whole-test retries as a fix. Web-first assertion polling is synchronization; repeated diagnostic runs preserve the original failure and report the observed failure rate.

## Test-first development

Work one vertical slice: public promise → real failing case → minimum fix → green. An edited case in an existing file satisfies test-first; a new file is optional. Refactor after green without changing its promise. Hand the scoped proof and diff to review.

Factory workers follow [worker discipline](../cas-worker/references/discipline.md): commit the red case, run its named filter, then commit the fix and run it green. Full integration proof stays with the established landing owner; read passing receipts instead of repeating checks.

## Stack seams

Use [tests.md](references/tests.md) for independent expectations, real Rust store/Git fixtures and one full-duration `TestEnvGuard` passed into helpers. Use [mocking.md](references/mocking.md) for predicate fakes, NestJS provider overrides and Nuxt composable Vitest seams. Keep the consumer real when replacing an external boundary.

Adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock; retain [LICENSE](LICENSE).

Done when the title's promise has an independent assertion, the meaningful sabotage run is red, restored code is green, and every changed module has its scoped proof or named integration owner.
